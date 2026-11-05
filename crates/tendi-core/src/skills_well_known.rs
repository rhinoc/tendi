use sha2::{Digest, Sha256};

use super::*;

pub(super) fn installed_source_version(root: &Path, target: &SkillTarget) -> Result<String> {
    let agent = target.agent_kind()?;
    let files = crate::skill_source::read_skill_files(root)?;
    source_version(&files, agent)
}

pub(crate) fn verified_source_snapshot(
    path: &SkillPath,
    version: &str,
) -> Result<Option<SkillSnapshot>> {
    let local = crate::skill_source::read_skill_files(&path.path)?;
    let mut source = local.clone();
    if crate::skill_source::skill_files_digest(&source) != version {
        source.retain(|name, _| !crate::providers::is_managed_skill_file(path.agent, name));
        if let Some(manifest) = source.get_mut("SKILL.md") {
            let mut text = std::str::from_utf8(manifest)?.to_string();
            for key in crate::providers::skill_frontmatter_visibility_keys(path.agent) {
                text = canonicalize_provider_visibility_key(&text, key, false);
            }
            *manifest = text.into_bytes();
        }
        if crate::skill_source::skill_files_digest(&source) != version {
            return Ok(None);
        }
    }
    Ok(Some(SkillSnapshot {
        skill_path: path.path.clone(),
        source_version: version.to_string(),
        files: local
            .into_iter()
            .map(|(relative_path, content)| SkillSnapshotFile {
                relative_path,
                content,
            })
            .collect(),
    }))
}

pub(super) fn check_update(
    skill: &SkillRecord,
    path: &SkillPath,
    cancelled: &AtomicBool,
) -> SkillUpdateReport {
    let mut report = SkillUpdateReport {
        id: ensure_skill_record_id(skill),
        name: skill.name.clone(),
        status: "missing-source".to_string(),
        current_version: path.source_version.clone(),
        latest_version: None,
        source: path.source.clone(),
        source_kind: path.source_kind.clone(),
        source_files: None,
    };
    let Some(source) = path.source.as_deref() else {
        return report;
    };
    if cancelled.load(Ordering::Acquire) {
        report.status = "cancelled".to_string();
        return report;
    }
    let result = (|| -> Result<_> {
        let incoming = crate::skill_source::read_well_known_skill(source, &skill.name)?;
        let latest = source_version(&incoming, path.agent)?;
        let local = source_version(
            &crate::skill_source::read_skill_files(&path.path)?,
            path.agent,
        )?;
        let current = path.source_version.clone().unwrap_or_else(|| local.clone());
        let matches = current == latest || local == latest;
        Ok((current, latest, matches, std::sync::Arc::new(incoming)))
    })();
    match result {
        Ok((current, latest, matches, files)) => {
            report.status = if cancelled.load(Ordering::Acquire) {
                "cancelled"
            } else if matches {
                "up-to-date"
            } else {
                "update-available"
            }
            .to_string();
            report.current_version = Some(current);
            report.latest_version = Some(latest);
            report.source_files = Some(files);
        }
        Err(error) => {
            crate::logging::global().warn(
                "well-known skill update check failed",
                serde_json::json!({ "skill": skill.name, "source": source, "error": error.to_string() }),
            );
            report.status = "unreachable".to_string();
        }
    }
    report
}

pub(super) fn plan_update(
    skill: &SkillRecord,
    path: &SkillPath,
    store: &crate::storage::Store,
    workspace: Option<&Path>,
    update: &SkillUpdateReport,
) -> Result<SkillDirectoryUpdate> {
    let source = path
        .source
        .as_deref()
        .context("well-known skill source is missing")?;
    let mut incoming = match &update.source_files {
        Some(files) => files.as_ref().clone(),
        None => {
            let incoming = crate::skill_source::read_well_known_skill(source, &skill.name)?;
            if Some(source_version(&incoming, path.agent)?).as_deref()
                != update.latest_version.as_deref()
            {
                bail!("skill source changed since the update check; check updates again");
            }
            incoming
        }
    };
    let local = crate::skill_source::read_skill_files(&path.path)?;
    let base = snapshot_files(store, path, "", workspace);
    if let Some(content) = incoming.get_mut("SKILL.md") {
        let local_manifest = local
            .get("SKILL.md")
            .and_then(|bytes| std::str::from_utf8(bytes).ok());
        let incoming_manifest =
            std::str::from_utf8(content).context("remote SKILL.md is not UTF-8")?;
        let (_, _, normalized) = normalize_skill_manifests_for_merge(
            local_manifest,
            None,
            Some(incoming_manifest),
            path.agent,
            path.effective_visibility,
        );
        *content = normalized
            .context("remote SKILL.md is missing")?
            .into_bytes();
    }
    let files = merge_file_maps(
        base,
        local,
        incoming,
        "",
        &path.path.to_string_lossy(),
        path.effective_visibility,
        path.agent,
    )
    .into_iter()
    .filter(|file| file.before_bytes != file.incoming_bytes)
    .collect();
    Ok(SkillDirectoryUpdate {
        name: skill.name.clone(),
        target: path.path.clone(),
        files,
    })
}

fn source_version(files: &BTreeMap<String, Vec<u8>>, agent: AgentKind) -> Result<String> {
    let mut hash = Sha256::new();
    for (path, bytes) in files {
        if crate::providers::is_managed_skill_file(agent, path) {
            continue;
        }
        hash.update(path.as_bytes());
        hash.update([0]);
        if path == "SKILL.md" {
            let mut manifest = MarkdownDoc::parse(std::str::from_utf8(bytes)?)?;
            manifest
                .meta
                .remove(Value::String("visibility".to_string()));
            for key in crate::providers::skill_frontmatter_visibility_keys(agent) {
                manifest.meta.remove(Value::String(key.to_string()));
            }
            hash.update(manifest.render()?.as_bytes());
        } else {
            hash.update(bytes);
        }
        hash.update([0]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
