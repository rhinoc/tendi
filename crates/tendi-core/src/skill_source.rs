use std::{
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const DISCOVERY_SCHEMA_V2: &str = "https://schemas.agentskills.io/discovery/0.2.0/schema.json";
const MAX_REMOTE_FILE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ARCHIVE_FILES: usize = 1000;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ParsedSkillSource {
    pub kind: String,
    pub local_root: Option<PathBuf>,
    pub url: String,
    pub git_ref: Option<String>,
    pub subpath: Option<PathBuf>,
}

pub(crate) fn parse(cwd: &Path, input: &str) -> Result<ParsedSkillSource> {
    let input = input.trim();
    if input.is_empty() {
        bail!("skill source must not be empty");
    }

    let candidate = PathBuf::from(input);
    let local_path = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };
    if local_path.exists() {
        return Ok(ParsedSkillSource {
            kind: "local".to_string(),
            local_root: Some(
                local_path
                    .canonicalize()
                    .with_context(|| format!("failed to canonicalize {}", local_path.display()))?,
            ),
            url: input.to_string(),
            git_ref: None,
            subpath: None,
        });
    }

    let (without_fragment, git_ref) = split_git_ref(input)?;
    let source = without_fragment.as_str();

    if let Some(rest) = source.strip_prefix("github:") {
        return parse_github_shorthand(rest, git_ref);
    }
    if let Some(rest) = source.strip_prefix("gitlab:") {
        return parse_gitlab_path("https", "gitlab.com", rest, git_ref);
    }

    if let Some(rest) = source
        .strip_prefix("https://github.com/")
        .or_else(|| source.strip_prefix("http://github.com/"))
    {
        return parse_github_path(rest, git_ref);
    }

    if let Some((scheme, rest)) = strip_http_scheme(source) {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host_lower = host.to_ascii_lowercase();
        let path_without_query = path.split(['?', '#']).next().unwrap_or(path);
        if host_lower == "gitlab.com" || path.contains("/-/tree/") {
            return parse_gitlab_path(scheme, host, path, git_ref);
        }
        if host_lower == "huggingface.co" {
            return parse_hugging_face(scheme, host, path, git_ref);
        }
        if host_lower == "clawhub.ai" && path_without_query == "api/v1/download" {
            return Ok(ParsedSkillSource {
                kind: "clawhub".to_string(),
                local_root: None,
                url: source.to_string(),
                git_ref: None,
                subpath: None,
            });
        }
        if source.ends_with(".git") {
            return Ok(git_source(source, classify_git_host(host), git_ref));
        }
        return Ok(ParsedSkillSource {
            kind: "well-known".to_string(),
            local_root: None,
            url: source.to_string(),
            git_ref: None,
            subpath: None,
        });
    }

    if source.starts_with("git@") || source.starts_with("ssh://") {
        let kind = if source.to_ascii_lowercase().contains("github") {
            "github"
        } else if source.to_ascii_lowercase().contains("gitlab") {
            "gitlab"
        } else if source.to_ascii_lowercase().contains("huggingface") {
            "huggingface"
        } else {
            "git"
        };
        return Ok(git_source(source, kind, git_ref));
    }

    if let Some(parsed) = parse_github_shorthand_parts(source, git_ref.clone())? {
        return Ok(parsed);
    }

    if source.ends_with(".git") {
        return Ok(git_source(source, "git", git_ref));
    }

    bail!("unsupported skill source {input:?}")
}

fn split_git_ref(input: &str) -> Result<(String, Option<String>)> {
    let Some((source, fragment)) = input.split_once('#') else {
        return Ok((input.to_string(), None));
    };
    if fragment.is_empty() {
        bail!("git ref must not be empty");
    }
    if !looks_like_git_source(source) {
        return Ok((input.to_string(), None));
    }
    let decoded = percent_decode(fragment)?;
    validate_git_ref(&decoded)?;
    Ok((source.to_string(), Some(decoded)))
}

fn looks_like_git_source(source: &str) -> bool {
    source.starts_with("github:")
        || source.starts_with("gitlab:")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.ends_with(".git")
        || source.contains("github.com/")
        || source.contains("gitlab.com/")
        || source.contains("huggingface.co/")
        || (!source.contains(':') && source.split('/').count() >= 2)
}

fn validate_git_ref(value: &str) -> Result<()> {
    let invalid_char = value
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || "~^:?*[\\".contains(ch));
    let invalid_segment = value
        .split('/')
        .any(|part| part.is_empty() || part == "." || part.ends_with('.'));
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.starts_with('.')
        || value.ends_with('/')
        || value.ends_with(".lock")
        || value.contains("..")
        || value.contains("@{")
        || invalid_char
        || invalid_segment
    {
        bail!("invalid git ref {value:?}");
    }
    Ok(())
}

fn percent_decode(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                bail!("invalid percent encoding in git ref");
            }
            let high = hex(bytes[index + 1]).context("invalid percent encoding in git ref")?;
            let low = hex(bytes[index + 2]).context("invalid percent encoding in git ref")?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).context("git ref is not valid UTF-8")
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_github_path(rest: &str, fragment_ref: Option<String>) -> Result<ParsedSkillSource> {
    let clean = rest.split('?').next().unwrap_or(rest).trim_end_matches('/');
    let parts = clean
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        bail!("GitHub URL must include owner and repository");
    }
    validate_repo_segment(parts[0], "GitHub owner")?;
    let repo = parts[1].trim_end_matches(".git");
    validate_repo_segment(repo, "GitHub repository")?;

    let (git_ref, subpath) = if parts.get(2) == Some(&"tree") {
        let tree_ref = parts.get(3).context("GitHub tree URL must include a ref")?;
        let tree_ref = percent_decode(tree_ref)?;
        validate_git_ref(&tree_ref)?;
        let subpath = path_from_segments(&parts[4..])?;
        (Some(tree_ref), subpath)
    } else {
        (fragment_ref, None)
    };
    Ok(ParsedSkillSource {
        kind: "github".to_string(),
        local_root: None,
        url: format!("https://github.com/{}/{repo}.git", parts[0]),
        git_ref,
        subpath,
    })
}

fn parse_github_shorthand(source: &str, git_ref: Option<String>) -> Result<ParsedSkillSource> {
    parse_github_shorthand_parts(source, git_ref)?
        .with_context(|| format!("invalid GitHub shorthand {source:?}"))
}

fn parse_github_shorthand_parts(
    source: &str,
    git_ref: Option<String>,
) -> Result<Option<ParsedSkillSource>> {
    if source.contains(':') || source.starts_with('.') || source.starts_with('/') {
        return Ok(None);
    }
    let parts = source
        .trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return Ok(None);
    }
    validate_repo_segment(parts[0], "GitHub owner")?;
    validate_repo_segment(parts[1].trim_end_matches(".git"), "GitHub repository")?;
    let subpath = path_from_segments(&parts[2..])?;
    Ok(Some(ParsedSkillSource {
        kind: "github".to_string(),
        local_root: None,
        url: format!(
            "https://github.com/{}/{}.git",
            parts[0],
            parts[1].trim_end_matches(".git")
        ),
        git_ref,
        subpath,
    }))
}

fn parse_gitlab_path(
    scheme: &str,
    host: &str,
    path: &str,
    fragment_ref: Option<String>,
) -> Result<ParsedSkillSource> {
    let clean = path.split('?').next().unwrap_or(path).trim_matches('/');
    let (repo_path, tree) = clean
        .split_once("/-/tree/")
        .map_or((clean, None), |(repo, tree)| (repo, Some(tree)));
    let repo_path = repo_path.trim_end_matches(".git");
    validate_repo_path(repo_path, "GitLab repository")?;
    let (git_ref, subpath) = if let Some(tree) = tree {
        let mut parts = tree.split('/').filter(|part| !part.is_empty());
        let tree_ref = parts.next().context("GitLab tree URL must include a ref")?;
        let tree_ref = percent_decode(tree_ref)?;
        validate_git_ref(&tree_ref)?;
        let remaining = parts.collect::<Vec<_>>();
        (Some(tree_ref), path_from_segments(&remaining)?)
    } else {
        (fragment_ref, None)
    };
    Ok(ParsedSkillSource {
        kind: "gitlab".to_string(),
        local_root: None,
        url: format!("{scheme}://{host}/{repo_path}.git"),
        git_ref,
        subpath,
    })
}

fn parse_hugging_face(
    scheme: &str,
    host: &str,
    path: &str,
    git_ref: Option<String>,
) -> Result<ParsedSkillSource> {
    let clean = path.split('?').next().unwrap_or(path).trim_matches('/');
    let (repository, tree_ref, subpath) =
        if let Some((repository, tree)) = clean.split_once("/tree/") {
            let mut parts = tree.split('/').filter(|part| !part.is_empty());
            let tree_ref = percent_decode(
                parts
                    .next()
                    .context("Hugging Face tree URL must include a ref")?,
            )?;
            validate_git_ref(&tree_ref)?;
            if git_ref.is_some() && git_ref.as_deref() != Some(tree_ref.as_str()) {
                bail!("Hugging Face tree URL and fragment specify different refs");
            }
            let remaining = parts.collect::<Vec<_>>();
            (repository, Some(tree_ref), path_from_segments(&remaining)?)
        } else {
            (clean, git_ref, None)
        };
    let segments = repository.split('/').collect::<Vec<_>>();
    let valid_len = segments.len() == 2
        || (segments.len() == 3 && matches!(segments[0], "datasets" | "spaces"));
    if !valid_len {
        bail!("Hugging Face source must identify a repository");
    }
    validate_repo_path(
        repository.trim_end_matches(".git"),
        "Hugging Face repository",
    )?;
    Ok(ParsedSkillSource {
        kind: "huggingface".to_string(),
        local_root: None,
        url: format!(
            "{scheme}://{host}/{}.git",
            repository.trim_end_matches(".git")
        ),
        git_ref: tree_ref,
        subpath,
    })
}

fn git_source(source: &str, kind: &str, git_ref: Option<String>) -> ParsedSkillSource {
    ParsedSkillSource {
        kind: kind.to_string(),
        local_root: None,
        url: source.to_string(),
        git_ref,
        subpath: None,
    }
}

fn strip_http_scheme(value: &str) -> Option<(&str, &str)> {
    value
        .strip_prefix("https://")
        .map(|rest| ("https", rest))
        .or_else(|| value.strip_prefix("http://").map(|rest| ("http", rest)))
}

fn classify_git_host(host: &str) -> &'static str {
    if host.eq_ignore_ascii_case("github.com") {
        "github"
    } else if host.eq_ignore_ascii_case("gitlab.com") || host.contains("gitlab.") {
        "gitlab"
    } else if host.eq_ignore_ascii_case("huggingface.co") {
        "huggingface"
    } else {
        "git"
    }
}

fn validate_repo_segment(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("invalid {label} {value:?}");
    }
    Ok(())
}

fn validate_repo_path(value: &str, label: &str) -> Result<()> {
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() < 2 {
        bail!("{label} must include an owner and repository");
    }
    for part in parts {
        validate_repo_segment(part, label)?;
    }
    Ok(())
}

fn path_from_segments(parts: &[&str]) -> Result<Option<PathBuf>> {
    if parts.is_empty() {
        return Ok(None);
    }
    let joined = parts.join("/");
    validate_relative_path(&joined)?;
    Ok(Some(PathBuf::from(joined)))
}

pub(crate) fn validate_relative_path(value: &str) -> Result<()> {
    if value.is_empty() || value.contains('\0') || value.contains('\\') {
        bail!("unsafe source subpath {value:?}");
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("unsafe source subpath {value:?}");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct DiscoveryIndex {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    skills: Vec<DiscoveryEntry>,
}

#[derive(Debug, Deserialize)]
struct DiscoveryEntry {
    name: String,
    description: String,
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    url: Option<String>,
    digest: Option<String>,
    files: Option<Vec<String>>,
}

pub(crate) fn materialize_well_known(source: &str, target: &Path) -> Result<()> {
    if source
        .strip_prefix("https://clawhub.ai/api/v1/download")
        .is_some()
    {
        let bytes = fetch_bytes(source)?;
        extract_skill_archive(&bytes, source, target)?;
        return Ok(());
    }

    let candidates = discovery_index_candidates(source)?;
    for index_url in candidates {
        let Some(index_text) = fetch_text_optional(&index_url)? else {
            continue;
        };
        let Ok(index) = serde_json::from_str::<DiscoveryIndex>(&index_text) else {
            continue;
        };
        if materialize_index(&index_url, &index, target)? {
            return Ok(());
        }
    }

    // Match skills CLI's direct-download fallback for a standalone SKILL.md URL
    // and registry endpoints that return a skill ZIP directly (for example
    // ClawHub's /api/v1/download endpoint).
    if let Some(bytes) = fetch_bytes_optional(source)? {
        if bytes.starts_with(b"PK")
            || bytes.starts_with(&[0x1f, 0x8b])
            || source.to_ascii_lowercase().ends_with(".zip")
            || source.to_ascii_lowercase().ends_with(".tar.gz")
            || source.to_ascii_lowercase().ends_with(".tgz")
        {
            extract_skill_archive(&bytes, source, target)?;
            return Ok(());
        }
        if let Ok(content) = String::from_utf8(bytes) {
            if content.contains("---") && content.contains("name:") {
                fs::create_dir_all(target)?;
                fs::write(target.join("SKILL.md"), content)?;
                return Ok(());
            }
        }
    }
    bail!(
        "no skills found at {source}; expected /.well-known/agent-skills/index.json, /.well-known/skills/index.json, or a SKILL.md URL"
    )
}

fn discovery_index_candidates(source: &str) -> Result<Vec<String>> {
    let (scheme, rest) = strip_http_scheme(source).context("well-known source must use HTTP(S)")?;
    let (authority, raw_path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty() || authority.contains('@') {
        bail!("invalid well-known source URL");
    }
    let path = raw_path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let origin = format!("{scheme}://{authority}");
    if path.ends_with("/index.json")
        && (path.contains(".well-known/agent-skills") || path.contains(".well-known/skills"))
    {
        return Ok(vec![format!("{origin}/{path}")]);
    }
    for endpoint in [".well-known/agent-skills", ".well-known/skills"] {
        if let Some(index) = path.find(endpoint) {
            let prefix = path[..index].trim_matches('/');
            let base = if prefix.is_empty() {
                origin.clone()
            } else {
                format!("{origin}/{prefix}")
            };
            return Ok(vec![format!("{base}/{endpoint}/index.json")]);
        }
    }
    let mut bases = Vec::new();
    bases.push(if path.is_empty() {
        origin.clone()
    } else {
        format!("{origin}/{path}")
    });
    if !path.is_empty() {
        bases.push(origin);
    }
    let mut result = Vec::new();
    for base in bases {
        for endpoint in [".well-known/agent-skills", ".well-known/skills"] {
            let candidate = format!("{}/{endpoint}/index.json", base.trim_end_matches('/'));
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
    }
    Ok(result)
}

fn materialize_index(index_url: &str, index: &DiscoveryIndex, target: &Path) -> Result<bool> {
    if index.skills.is_empty() {
        return Ok(false);
    }
    let is_v2 = index.schema.as_deref() == Some(DISCOVERY_SCHEMA_V2);
    if index.schema.is_some() && !is_v2 {
        return Ok(false);
    }
    let base = index_url.trim_end_matches("index.json");
    let mut installed = 0;
    for entry in &index.skills {
        validate_skill_name(&entry.name)?;
        if entry.description.is_empty() || entry.description.len() > 1024 {
            bail!("invalid well-known description for {}", entry.name);
        }
        let skill_root = target.join(&entry.name);
        if is_v2 {
            let artifact = entry
                .url
                .as_deref()
                .context("well-known artifact URL is missing")?;
            let artifact_url = resolve_http_url(index_url, artifact)?;
            let bytes = fetch_bytes(&artifact_url)?;
            verify_digest(&bytes, entry.digest.as_deref())?;
            match entry.artifact_type.as_deref() {
                Some("skill-md") => {
                    fs::create_dir_all(&skill_root)?;
                    fs::write(skill_root.join("SKILL.md"), bytes)?;
                }
                Some("archive") => extract_skill_archive(&bytes, &artifact_url, &skill_root)?,
                _ => continue,
            }
            installed += 1;
        } else {
            let files = entry
                .files
                .as_ref()
                .context("legacy well-known files are missing")?;
            if !files
                .iter()
                .any(|file| file.eq_ignore_ascii_case("SKILL.md"))
            {
                bail!("well-known skill {} does not include SKILL.md", entry.name);
            }
            for file in files {
                validate_relative_path(file)?;
            }
            for file in files {
                let bytes = fetch_bytes(&format!("{base}{}/{file}", entry.name))?;
                let destination = skill_root.join(file);
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(destination, bytes)?;
            }
            installed += 1;
        }
    }
    Ok(installed > 0)
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        bail!("invalid well-known skill name {name:?}");
    }
    Ok(())
}

fn resolve_http_url(base: &str, value: &str) -> Result<String> {
    if value.starts_with("https://") || value.starts_with("http://") {
        return Ok(value.to_string());
    }
    if value.starts_with("//") {
        let scheme = base
            .split_once("://")
            .map(|(scheme, _)| scheme)
            .unwrap_or("https");
        return Ok(format!("{scheme}:{value}"));
    }
    let (scheme, rest) = strip_http_scheme(base).context("invalid artifact base URL")?;
    let (authority, _) = rest.split_once('/').unwrap_or((rest, ""));
    if value.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{value}"));
    }
    let parent = base
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or(base);
    Ok(format!("{parent}/{value}"))
}

fn verify_digest(bytes: &[u8], expected: Option<&str>) -> Result<()> {
    let expected = expected.context("well-known artifact digest is missing")?;
    let Some(hex) = expected.strip_prefix("sha256:") else {
        bail!("well-known artifact digest must use sha256");
    };
    if hex.len() != 64
        || !hex
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    {
        bail!("invalid well-known artifact digest");
    }
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != hex {
        bail!("well-known artifact digest mismatch");
    }
    Ok(())
}

fn extract_skill_archive(bytes: &[u8], artifact_url: &str, target: &Path) -> Result<()> {
    if bytes.starts_with(b"PK") || artifact_url.to_ascii_lowercase().ends_with(".zip") {
        extract_zip(bytes, target)?;
    } else if bytes.starts_with(&[0x1f, 0x8b])
        || artifact_url.to_ascii_lowercase().ends_with(".tar.gz")
        || artifact_url.to_ascii_lowercase().ends_with(".tgz")
    {
        extract_tar_gz(bytes, target)?;
    } else {
        bail!("unsupported well-known archive format");
    }
    if !target.join("SKILL.md").is_file() {
        bail!("well-known archive is missing root SKILL.md");
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], target: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("invalid ZIP archive")?;
    if archive.len() > MAX_ARCHIVE_FILES {
        bail!("well-known archive contains too many files");
    }
    let total_size = (0..archive.len()).try_fold(0_u64, |total, index| {
        let file = archive.by_index(index).context("invalid ZIP entry")?;
        total
            .checked_add(file.size())
            .context("well-known archive size overflow")
    })?;
    if total_size > MAX_REMOTE_FILE_BYTES {
        bail!("well-known archive exceeds 50 MiB unpacked");
    }
    fs::create_dir_all(target)?;
    let mut extracted_size = 0_u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).context("invalid ZIP entry")?;
        let raw_name = file.name().to_string();
        validate_relative_path(raw_name.trim_end_matches('/'))?;
        let unix_mode = file.unix_mode().unwrap_or_default();
        if unix_mode & 0o170000 == 0o120000 {
            bail!("well-known archive links are not supported");
        }
        let destination = target.join(&raw_name);
        if file.is_dir() {
            fs::create_dir_all(&destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(&destination)?;
        let remaining = MAX_REMOTE_FILE_BYTES.saturating_sub(extracted_size);
        let written = std::io::copy(&mut file.take(remaining + 1), &mut output)?;
        extracted_size += written;
        if extracted_size > MAX_REMOTE_FILE_BYTES {
            bail!("well-known archive exceeds 50 MiB unpacked");
        }
    }
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], target: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut files = 0_usize;
    let mut total_size = 0_u64;
    fs::create_dir_all(target)?;
    for entry in archive.entries().context("invalid TAR.GZ archive")? {
        let mut entry = entry.context("invalid TAR entry")?;
        files += 1;
        if files > MAX_ARCHIVE_FILES {
            bail!("well-known archive contains too many files");
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            bail!("well-known archive links are not supported");
        }
        let path = entry.path().context("invalid TAR entry path")?;
        let raw_name = path.to_string_lossy();
        validate_relative_path(raw_name.trim_end_matches('/'))?;
        if entry_type.is_dir() {
            fs::create_dir_all(target.join(path.as_ref()))?;
            continue;
        }
        if !entry_type.is_file() {
            continue;
        }
        total_size = total_size
            .checked_add(entry.size())
            .context("well-known archive size overflow")?;
        if total_size > MAX_REMOTE_FILE_BYTES {
            bail!("well-known archive exceeds 50 MiB unpacked");
        }
        let destination = target.join(path.as_ref());
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(&destination)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

fn fetch_text_optional(url: &str) -> Result<Option<String>> {
    match fetch_bytes_optional(url)? {
        Some(bytes) => String::from_utf8(bytes)
            .map(Some)
            .with_context(|| format!("{url} did not return UTF-8 text")),
        None => Ok(None),
    }
}

fn fetch_bytes_optional(url: &str) -> Result<Option<Vec<u8>>> {
    match ureq::get(url).call() {
        Ok(response) => {
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(MAX_REMOTE_FILE_BYTES + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("failed to read {url}"))?;
            if bytes.len() as u64 > MAX_REMOTE_FILE_BYTES {
                bail!("remote skill file exceeds 50 MiB");
            }
            Ok(Some(bytes))
        }
        Err(ureq::Error::Status(_, _)) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to fetch {url}")),
    }
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    fetch_bytes_optional(url)?.with_context(|| format!("remote skill file not found: {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_github_tree_ref_subpath_and_fragment_ref() {
        let cwd = Path::new("/tmp/missing-tendi-source-parser");
        let tree = parse(cwd, "https://github.com/acme/skills/tree/main/skills/demo").unwrap();
        assert_eq!(tree.kind, "github");
        assert_eq!(tree.url, "https://github.com/acme/skills.git");
        assert_eq!(tree.git_ref.as_deref(), Some("main"));
        assert_eq!(tree.subpath.as_deref(), Some(Path::new("skills/demo")));

        let fragment = parse(cwd, "acme/skills/skills/demo#feature%2Finstall").unwrap();
        assert_eq!(fragment.git_ref.as_deref(), Some("feature/install"));
        assert_eq!(fragment.subpath.as_deref(), Some(Path::new("skills/demo")));
    }

    #[test]
    fn parses_clawhub_download_source() {
        let source = parse(
            Path::new("/tmp/missing-tendi-source-parser"),
            "https://clawhub.ai/api/v1/download?slug=demo&version=1.0.0",
        )
        .unwrap();
        assert_eq!(source.kind, "clawhub");
        assert_eq!(
            source.url,
            "https://clawhub.ai/api/v1/download?slug=demo&version=1.0.0"
        );
    }

    #[test]
    fn parses_gitlab_https_ssh_and_hugging_face() {
        let cwd = Path::new("/tmp/missing-tendi-source-parser");
        let gitlab = parse(
            cwd,
            "https://gitlab.com/group/sub/repo/-/tree/release/skills/demo",
        )
        .unwrap();
        assert_eq!(gitlab.kind, "gitlab");
        assert_eq!(gitlab.url, "https://gitlab.com/group/sub/repo.git");
        assert_eq!(gitlab.git_ref.as_deref(), Some("release"));
        assert_eq!(gitlab.subpath.as_deref(), Some(Path::new("skills/demo")));

        let ssh = parse(cwd, "git@gitlab.com:group/repo.git#v1.2.0").unwrap();
        assert_eq!(ssh.kind, "gitlab");
        assert_eq!(ssh.git_ref.as_deref(), Some("v1.2.0"));

        let hf = parse(
            cwd,
            "https://huggingface.co/datasets/acme/skills/tree/main/skills/demo",
        )
        .unwrap();
        assert_eq!(hf.kind, "huggingface");
        assert_eq!(hf.url, "https://huggingface.co/datasets/acme/skills.git");
        assert_eq!(hf.git_ref.as_deref(), Some("main"));
        assert_eq!(hf.subpath.as_deref(), Some(Path::new("skills/demo")));
    }

    #[test]
    fn rejects_path_traversal_and_unsafe_refs() {
        let cwd = Path::new("/tmp/missing-tendi-source-parser");
        for source in [
            "acme/skills/../secret",
            "https://github.com/acme/skills/tree/main/skills/../secret",
            "https://gitlab.com/acme/skills/-/tree/main/../../secret",
        ] {
            assert!(parse(cwd, source).is_err(), "accepted {source}");
        }
        for source in [
            "acme/skills#--upload-pack=evil",
            "acme/skills#refs/heads/main.lock",
            "acme/skills#main%20evil",
            "acme/skills#main..evil",
            "acme/skills#main%5Eevil",
        ] {
            assert!(parse(cwd, source).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn recognizes_well_known_and_builds_safe_candidates() {
        let cwd = Path::new("/tmp/missing-tendi-source-parser");
        let source = parse(cwd, "https://docs.example.com/product").unwrap();
        assert_eq!(source.kind, "well-known");
        assert_eq!(
            discovery_index_candidates(&source.url).unwrap(),
            vec![
                "https://docs.example.com/product/.well-known/agent-skills/index.json",
                "https://docs.example.com/product/.well-known/skills/index.json",
                "https://docs.example.com/.well-known/agent-skills/index.json",
                "https://docs.example.com/.well-known/skills/index.json",
            ]
        );
        assert_eq!(
            discovery_index_candidates("https://docs.example.com/.well-known/agent-skills/demo")
                .unwrap(),
            vec!["https://docs.example.com/.well-known/agent-skills/index.json"]
        );
        assert!(discovery_index_candidates("https://user@example.com").is_err());
    }

    #[test]
    fn validates_v2_digest_and_legacy_paths() {
        let bytes = b"---\nname: demo\ndescription: Demo\n---\n";
        let digest = format!("sha256:{:x}", Sha256::digest(bytes));
        verify_digest(bytes, Some(&digest)).unwrap();
        assert!(verify_digest(b"changed", Some(&digest)).is_err());
        assert!(validate_relative_path("references/guide.md").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("references\\secret").is_err());
        assert!(validate_skill_name("demo-skill").is_ok());
        assert!(validate_skill_name("../demo").is_err());
    }

    #[test]
    fn rejects_unsafe_legacy_index_before_downloading_files() {
        let index: DiscoveryIndex = serde_json::from_value(serde_json::json!({
            "skills": [{
                "name": "demo",
                "description": "Demo",
                "files": ["SKILL.md", "../secret"]
            }]
        }))
        .unwrap();
        let target =
            std::env::temp_dir().join(format!("tendi-well-known-security-{}", std::process::id()));
        let error = materialize_index(
            "https://invalid.example/.well-known/agent-skills/index.json",
            &index,
            &target,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsafe source subpath"));
        assert!(!target.exists());
    }

    #[test]
    fn extracts_safe_zip_and_rejects_traversal() {
        let skill = b"---\nname: demo\ndescription: Demo\n---\n";
        let mut safe = zip::ZipWriter::new(Cursor::new(Vec::new()));
        safe.start_file("SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        safe.write_all(skill).unwrap();
        safe.start_file(
            "references/guide.md",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        safe.write_all(b"Guide").unwrap();
        let safe = safe.finish().unwrap().into_inner();
        let target = std::env::temp_dir().join(format!(
            "tendi-well-known-zip-{}-{}",
            std::process::id(),
            safe.len()
        ));
        extract_skill_archive(&safe, "https://example.com/demo.zip", &target).unwrap();
        assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), skill);
        assert_eq!(
            fs::read_to_string(target.join("references/guide.md")).unwrap(),
            "Guide"
        );
        let _ = fs::remove_dir_all(&target);

        let mut unsafe_zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        unsafe_zip
            .start_file("../secret", zip::write::SimpleFileOptions::default())
            .unwrap();
        unsafe_zip.write_all(b"secret").unwrap();
        let unsafe_zip = unsafe_zip.finish().unwrap().into_inner();
        assert!(extract_zip(&unsafe_zip, &target).is_err());
        assert!(!target.join("secret").exists());
        let _ = fs::remove_dir_all(target);
    }
}
