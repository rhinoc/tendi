//! settings persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub fn save_skill_backup_config(
        &self,
        config: &crate::skill_backup::BackupConfig,
    ) -> Result<crate::skill_backup::BackupConfig> {
        self.with_named_write_transaction("save_skill_backup_config", |tx| {
            config.validate()?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('skill_backup_config', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![serde_json::to_string(config)?],
            )?;
            Ok(config.clone())
        })
    }

    pub fn clear_skill_backup_config(&self) -> Result<bool> {
        self.with_named_write_transaction("clear_skill_backup_config", |tx| {
            Ok(tx.execute(
                "DELETE FROM app_settings WHERE key = 'skill_backup_config'",
                [],
            )? > 0)
        })
    }

    pub fn app_settings(&self) -> Result<AppSettings> {
        with_database_read_lock_retry(|| self.app_settings_once())
    }

    pub(in crate::storage) fn app_settings_once(&self) -> Result<AppSettings> {
        Self::app_settings_on(&self.conn)
    }

    fn app_settings_on(conn: &Connection) -> Result<AppSettings> {
        // One statement observes one SQLite snapshot, including across processes.
        let mut statement = conn.prepare("SELECT key, value FROM app_settings")?;
        let values = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        let value = |key: &str| values.get(key).map(String::as_str);
        Ok(AppSettings {
            appearance: normalize_appearance(value("appearance").unwrap_or(&default_appearance()))?,
            font_family: normalize_font_family(
                value("font_family").unwrap_or(&default_font_family()),
            )?,
            light_theme: normalize_color_theme(
                value("light_theme").unwrap_or(&default_color_theme()),
            )?,
            dark_theme: normalize_color_theme(
                value("dark_theme").unwrap_or(&default_color_theme()),
            )?,
            app_icon: normalize_color_theme(value("app_icon").unwrap_or(&default_app_icon()))?,
            terminal: value("terminal").unwrap_or("auto").to_owned(),
            session_resume_target: normalize_session_resume_target(
                value("session_resume_target").unwrap_or(&default_session_resume_target()),
            )?,
            missing_session_project_policy: normalize_missing_session_project_policy(
                value("missing_session_project_policy")
                    .unwrap_or(&default_missing_session_project_policy()),
            )?,
            editor: value("editor").unwrap_or(&default_editor()).to_owned(),
            developer_mode: value("developer_mode")
                .map(serde_json::from_str)
                .transpose()
                .context("invalid developer mode setting")?
                .unwrap_or(false),
            additional_session_roots: value("additional_session_roots")
                .map(serde_json::from_str)
                .transpose()
                .context("invalid additional session roots setting")?
                .unwrap_or_default(),
            config_profiles: value("config_profiles")
                .map(serde_json::from_str)
                .transpose()
                .context("invalid config profiles setting")?
                .unwrap_or_default(),
        })
    }

    pub fn patch_app_settings(&self, patch: AppSettingsPatch) -> Result<AppSettings> {
        let mut values = Vec::<(&str, String)>::new();
        if let Some(value) = patch.appearance {
            values.push(("appearance", normalize_appearance(&value)?));
        }
        if let Some(value) = patch.font_family {
            values.push(("font_family", normalize_font_family(&value)?));
        }
        if let Some(value) = patch.light_theme {
            values.push(("light_theme", normalize_color_theme(&value)?));
        }
        if let Some(value) = patch.dark_theme {
            values.push(("dark_theme", normalize_color_theme(&value)?));
        }
        if let Some(value) = patch.app_icon {
            values.push(("app_icon", normalize_color_theme(&value)?));
        }
        if let Some(value) = patch.terminal {
            values.push(("terminal", normalize_setting_value(&value, "auto")));
        }
        if let Some(value) = patch.editor {
            values.push(("editor", normalize_setting_value(&value, "vscode")));
        }
        if let Some(value) = patch.session_resume_target {
            values.push((
                "session_resume_target",
                normalize_session_resume_target(&value)?,
            ));
        }
        if let Some(value) = patch.missing_session_project_policy {
            values.push((
                "missing_session_project_policy",
                normalize_missing_session_project_policy(&value)?,
            ));
        }
        if let Some(value) = patch.developer_mode {
            values.push(("developer_mode", serde_json::to_string(&value)?));
        }
        if let Some(value) = patch.additional_session_roots {
            values.push((
                "additional_session_roots",
                serde_json::to_string(&normalize_additional_session_roots(value)?)?,
            ));
        }
        self.with_named_write_transaction("patch_app_settings", |tx| {
            for (key, value) in values {
                tx.execute("INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
            }
            Self::app_settings_on(tx)
        })
    }

    pub fn set_config_profile(&self, key: &str, profile: Option<&str>) -> Result<AppSettings> {
        self.with_named_write_transaction("set_config_profile", |tx| {
            let mut settings = Self::app_settings_on(tx)?;
            if let Some(profile) = profile {
                settings
                    .config_profiles
                    .insert(key.to_owned(), profile.to_owned());
            } else {
                settings.config_profiles.remove(key);
            }
            Self::write_config_profiles(tx, &settings.config_profiles)?;
            Ok(settings)
        })
    }

    pub fn clear_config_profiles_if_matching(
        &self,
        deleted: &[(String, String)],
    ) -> Result<AppSettings> {
        self.with_named_write_transaction("clear_config_profiles_if_matching", |tx| {
            let mut settings = Self::app_settings_on(tx)?;
            let count = settings.config_profiles.len();
            settings.config_profiles.retain(|key, profile| {
                !deleted.iter().any(|(deleted_key, deleted_profile)| {
                    key == deleted_key && profile == deleted_profile
                })
            });
            if settings.config_profiles.len() != count {
                Self::write_config_profiles(tx, &settings.config_profiles)?;
            }
            Ok(settings)
        })
    }

    fn write_config_profiles(conn: &Connection, profiles: &BTreeMap<String, String>) -> Result<()> {
        conn.execute("INSERT INTO app_settings (key, value) VALUES ('config_profiles', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [serde_json::to_string(profiles)?])?;
        Ok(())
    }

    pub fn save_app_settings(&self, settings: AppSettings) -> Result<AppSettings> {
        let appearance = normalize_appearance(&settings.appearance)?;
        let font_family = normalize_font_family(&settings.font_family)?;
        let light_theme = normalize_color_theme(&settings.light_theme)?;
        let dark_theme = normalize_color_theme(&settings.dark_theme)?;
        let app_icon = normalize_color_theme(&settings.app_icon)?;
        let terminal = normalize_setting_value(&settings.terminal, "auto");
        let session_resume_target =
            normalize_session_resume_target(&settings.session_resume_target)?;
        let missing_session_project_policy =
            normalize_missing_session_project_policy(&settings.missing_session_project_policy)?;
        let editor = normalize_setting_value(&settings.editor, "vscode");
        let developer_mode = settings.developer_mode;
        let additional_session_roots =
            normalize_additional_session_roots(settings.additional_session_roots)?;
        let config_profiles = settings
            .config_profiles
            .into_iter()
            .filter_map(|(agent, profile)| {
                let agent = agent.trim();
                let profile = profile.trim();
                (!agent.is_empty() && !profile.is_empty())
                    .then(|| (agent.to_string(), profile.to_string()))
            })
            .collect::<BTreeMap<_, _>>();
        self.with_named_write_transaction("save_app_settings", |tx| {
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('appearance', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![appearance],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('font_family', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![font_family],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('light_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![light_theme],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('dark_theme', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![dark_theme],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('app_icon', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![app_icon],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('terminal', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![terminal],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('session_resume_target', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![session_resume_target],
            )?;
            tx.execute(
            "INSERT INTO app_settings (key, value) VALUES ('missing_session_project_policy', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![missing_session_project_policy],
        )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('editor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![editor],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('developer_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![serde_json::to_string(&developer_mode)?],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('additional_session_roots', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![serde_json::to_string(&additional_session_roots)?],
            )?;
            tx.execute(
                "INSERT INTO app_settings (key, value) VALUES ('config_profiles', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![serde_json::to_string(&config_profiles)?],
            )?;
            Ok(())
        })?;
        Ok(AppSettings {
            appearance,
            font_family,
            light_theme,
            dark_theme,
            app_icon,
            terminal,
            session_resume_target,
            missing_session_project_policy,
            editor,
            developer_mode,
            additional_session_roots,
            config_profiles,
        })
    }

    pub fn skill_backup_config(&self) -> Result<Option<crate::skill_backup::BackupConfig>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'skill_backup_config'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        value
            .map(|value| serde_json::from_str(&value).context("invalid skill backup configuration"))
            .transpose()
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
