use std::{
    collections::HashSet,
    env,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Duration, Local};
use serde_json::Value;

const DEFAULT_MAX_SIZE_BYTES: u64 = 50 * 1024 * 1024;
const DEFAULT_MAX_BACKUPS: usize = 10;
const DEFAULT_MAX_AGE_DAYS: i64 = 14;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 300 * 1024 * 1024;

static GLOBAL_LOGGER: OnceLock<Logger> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            value => bail!("invalid log level {value:?}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Clone)]
pub struct Logger {
    inner: Arc<LoggerInner>,
}

struct LoggerInner {
    component: String,
    min_level: Level,
    writer: Mutex<Option<RotatingFileWriter>>,
}

impl Logger {
    pub fn path() -> Result<PathBuf> {
        resolve_log_path()
    }

    fn new(component: &str) -> Result<Self> {
        let path = resolve_log_path()?;
        let writer = RotatingFileWriter::open(path, RotationConfig::from_env())?;
        let min_level = env::var("TENDI_LOG_LEVEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| Level::parse(&value))
            .transpose()?
            .unwrap_or(Level::Info);
        Ok(Self {
            inner: Arc::new(LoggerInner {
                component: component.to_string(),
                min_level,
                writer: Mutex::new(Some(writer)),
            }),
        })
    }

    fn disabled(component: &str) -> Self {
        Self {
            inner: Arc::new(LoggerInner {
                component: component.to_string(),
                min_level: Level::Info,
                writer: Mutex::new(None),
            }),
        }
    }

    pub fn log(&self, level: Level, message: &str, fields: Value) -> Result<()> {
        if level < self.inner.min_level {
            return Ok(());
        }

        let line = format_log_line(&self.inner.component, level, message, fields)?;
        let mut writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| anyhow!("log writer lock is poisoned"))?;
        if let Some(writer) = writer.as_mut() {
            writer.write(line.as_bytes())?;
        }
        Ok(())
    }

    pub fn debug(&self, message: &str, fields: Value) {
        let _ = self.log(Level::Debug, message, fields);
    }

    pub fn info(&self, message: &str, fields: Value) {
        let _ = self.log(Level::Info, message, fields);
    }

    pub fn warn(&self, message: &str, fields: Value) {
        let _ = self.log(Level::Warn, message, fields);
    }

    pub fn error(&self, message: &str, fields: Value) {
        let _ = self.log(Level::Error, message, fields);
    }
}

pub fn init(component: &str) -> Result<Logger> {
    if let Some(logger) = GLOBAL_LOGGER.get() {
        return Ok(logger.clone());
    }
    let logger = Logger::new(component)?;
    let _ = GLOBAL_LOGGER.set(logger.clone());
    Ok(GLOBAL_LOGGER.get().cloned().unwrap_or(logger))
}

pub fn global() -> Logger {
    GLOBAL_LOGGER
        .get_or_init(|| Logger::new("tendi").unwrap_or_else(|_| Logger::disabled("tendi")))
        .clone()
}

pub fn log_event(level: &str, message: &str, fields: Value) -> Result<()> {
    let level = Level::parse(level)?;
    global().log(level, message, fields)
}

fn resolve_log_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("TENDI_LOG_PATH").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let dir = env::var_os("TENDI_LOG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            dirs::data_dir()
                .or_else(|| dirs::home_dir().map(|home| home.join("Library/Application Support")))
                .map(|base| base.join("tendi/logs"))
        })
        .context("could not resolve log directory")?;
    Ok(dir.join("tendi.log"))
}

fn format_log_line(component: &str, level: Level, message: &str, fields: Value) -> Result<String> {
    let mut parts = vec![
        format!("time={}", Local::now().to_rfc3339()),
        format!("level={}", level.as_str()),
        format!("component={}", json_string(component)?),
        format!("pid={}", std::process::id()),
        format!("msg={}", json_string(message)?),
    ];

    if let Value::Object(fields) = fields {
        for (key, value) in fields {
            let key = sanitize_key(&key);
            if !key.is_empty() {
                parts.push(format!("{}={}", key, serde_json::to_string(&value)?));
            }
        }
    } else if !fields.is_null() {
        parts.push(format!("fields={}", serde_json::to_string(&fields)?));
    }

    Ok(format!("{}\n", parts.join(" ")))
}

fn json_string(value: &str) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn sanitize_key(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
struct RotationConfig {
    max_size_bytes: u64,
    max_backups: usize,
    max_age_days: i64,
    max_total_bytes: u64,
}

impl RotationConfig {
    fn from_env() -> Self {
        Self {
            max_size_bytes: env_u64(
                "TENDI_LOG_MAX_SIZE_MB",
                DEFAULT_MAX_SIZE_BYTES / 1024 / 1024,
            ) * 1024
                * 1024,
            max_backups: env_usize("TENDI_LOG_MAX_BACKUPS", DEFAULT_MAX_BACKUPS),
            max_age_days: env_i64("TENDI_LOG_MAX_AGE_DAYS", DEFAULT_MAX_AGE_DAYS),
            max_total_bytes: env_u64(
                "TENDI_LOG_MAX_TOTAL_MB",
                DEFAULT_MAX_TOTAL_BYTES / 1024 / 1024,
            ) * 1024
                * 1024,
        }
    }
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

fn env_usize(key: &str, fallback: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

fn env_i64(key: &str, fallback: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(fallback)
}

struct RotatingFileWriter {
    active_path: PathBuf,
    dir: PathBuf,
    prefix: String,
    suffix: String,
    config: RotationConfig,
    file: Option<File>,
    current_day: String,
    current_size: u64,
}

impl RotatingFileWriter {
    fn open(path: PathBuf, config: RotationConfig) -> Result<Self> {
        let path = path
            .canonicalize()
            .or_else(|_| Ok::<PathBuf, std::io::Error>(path.clone()))?;
        let dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("log path has no file name")?;
        let suffix = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        let prefix = if suffix.is_empty() {
            file_name.to_string()
        } else {
            file_name.trim_end_matches(&suffix).to_string()
        };
        let mut writer = Self {
            active_path: path,
            dir,
            prefix,
            suffix,
            config,
            file: None,
            current_day: String::new(),
            current_size: 0,
        };
        writer.open_active()?;
        writer.cleanup(&today());
        Ok(writer)
    }

    fn write(&mut self, content: &[u8]) -> Result<()> {
        let day = today();
        self.ensure_ready(&day, content.len() as u64)?;
        let file = self.file.as_mut().context("log file is not open")?;
        file.write_all(content)?;
        file.flush()?;
        self.current_size += content.len() as u64;
        Ok(())
    }

    fn ensure_ready(&mut self, day: &str, write_len: u64) -> Result<()> {
        if self.file.is_none() {
            self.open_active()?;
        }
        if self.current_size > 0 && self.current_day != day {
            self.rotate_active(&self.current_day.clone())?;
        }
        if self.config.max_size_bytes > 0
            && self.current_size > 0
            && self.current_size + write_len > self.config.max_size_bytes
        {
            self.rotate_active(day)?;
        }
        Ok(())
    }

    fn rotate_active(&mut self, day: &str) -> Result<()> {
        self.file.take();
        if self.current_size > 0 {
            let rotated = self.next_rotated_path(day)?;
            if self.active_path.exists() {
                fs::rename(&self.active_path, rotated)?;
            }
        }
        self.open_active()?;
        self.cleanup(day);
        Ok(())
    }

    fn open_active(&mut self) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("create log directory {}", self.dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_path)
            .with_context(|| format!("open log file {}", self.active_path.display()))?;
        let metadata = file.metadata()?;
        self.current_size = metadata.len();
        self.current_day = if self.current_size == 0 {
            today()
        } else {
            metadata
                .modified()
                .map(day_for_time)
                .unwrap_or_else(|_| today())
        };
        self.file = Some(file);
        Ok(())
    }

    fn next_rotated_path(&self, day: &str) -> Result<PathBuf> {
        let used = fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .filter_map(|entry| {
                        self.parse_rotated_name(&entry.file_name().to_string_lossy())
                    })
                    .filter(|(entry_day, _)| entry_day == day)
                    .map(|(_, index)| index)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        for index in 0.. {
            if !used.contains(&index) {
                return Ok(self.dir.join(self.rotated_file_name(day, index)));
            }
        }
        unreachable!()
    }

    fn cleanup(&self, current_day: &str) {
        let mut files = self.rotated_files();
        let mut remove = HashSet::new();
        if self.config.max_age_days > 0 {
            let cutoff = (Local::now() - Duration::days(self.config.max_age_days))
                .format("%Y-%m-%d")
                .to_string();
            for file in &files {
                if file.day != current_day && file.day < cutoff {
                    remove.insert(file.path.clone());
                }
            }
        }

        files.retain(|file| !remove.contains(&file.path));
        files.sort_by(|left, right| {
            right
                .day
                .cmp(&left.day)
                .then_with(|| right.index.cmp(&left.index))
        });
        for file in files.iter().skip(self.config.max_backups) {
            remove.insert(file.path.clone());
        }
        for path in &remove {
            let _ = fs::remove_file(path);
        }

        if self.config.max_total_bytes == 0 {
            return;
        }
        let mut managed = self.rotated_files();
        if let Ok(metadata) = fs::metadata(&self.active_path) {
            managed.push(RotatedFile {
                path: self.active_path.clone(),
                day: current_day.to_string(),
                index: usize::MAX,
                size: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        let mut total = managed.iter().map(|file| file.size).sum::<u64>();
        managed.sort_by_key(|file| file.modified);
        for file in managed {
            if total <= self.config.max_total_bytes || file.index == usize::MAX {
                continue;
            }
            let _ = fs::remove_file(&file.path);
            total = total.saturating_sub(file.size);
        }
    }

    fn rotated_files(&self) -> Vec<RotatedFile> {
        fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                let (day, index) = self.parse_rotated_name(&name)?;
                let metadata = entry.metadata().ok()?;
                Some(RotatedFile {
                    path: entry.path(),
                    day,
                    index,
                    size: metadata.len(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                })
            })
            .collect()
    }

    fn rotated_file_name(&self, day: &str, index: usize) -> String {
        if index == 0 {
            format!("{}.{}{}", self.prefix, day, self.suffix)
        } else {
            format!("{}.{}.{}{}", self.prefix, day, index, self.suffix)
        }
    }

    fn parse_rotated_name(&self, name: &str) -> Option<(String, usize)> {
        let prefix = format!("{}.", self.prefix);
        if !name.starts_with(&prefix) || !name.ends_with(&self.suffix) {
            return None;
        }
        let value = name.strip_prefix(&prefix)?.strip_suffix(&self.suffix)?;
        let mut parts = value.split('.');
        let day = parts.next()?;
        if !day.chars().enumerate().all(|(index, character)| {
            character.is_ascii_digit() || (index == 4 || index == 7) && character == '-'
        }) || day.len() != 10
        {
            return None;
        }
        let index = parts
            .next()
            .map(|value| value.parse().ok())
            .flatten()
            .unwrap_or(0);
        if parts.next().is_some() {
            return None;
        }
        Some((day.to_string(), index))
    }
}

struct RotatedFile {
    path: PathBuf,
    day: String,
    index: usize,
    size: u64,
    modified: SystemTime,
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn day_for_time(value: SystemTime) -> String {
    chrono::DateTime::<Local>::from(value)
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn log_line_contains_structured_fields() {
        let line = format_log_line(
            "tendi-test",
            Level::Warn,
            "request failed",
            serde_json::json!({"request_id": "abc", "attempt": 2}),
        )
        .unwrap();
        assert!(line.contains("level=warn"));
        assert!(line.contains("component=\"tendi-test\""));
        assert!(line.contains("request_id=\"abc\""));
        assert!(line.contains("attempt=2"));
    }

    #[test]
    fn writer_rotates_when_size_budget_is_exceeded() {
        let directory = tempfile_directory();
        let path = directory.join("tendi.log");
        let mut writer = RotatingFileWriter::open(
            path.clone(),
            RotationConfig {
                max_size_bytes: 4,
                max_backups: 10,
                max_age_days: 0,
                max_total_bytes: 0,
            },
        )
        .unwrap();
        writer.write(b"1234").unwrap();
        writer.write(b"5").unwrap();
        assert!(
            directory
                .read_dir()
                .unwrap()
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.file_name().to_string_lossy().starts_with("tendi."))
        );
        let _ = fs::remove_dir_all(directory);
    }

    fn tempfile_directory() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "tendi-logging-test-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
