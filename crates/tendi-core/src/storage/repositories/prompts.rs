//! prompts persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub fn list_prompts(&self) -> Result<Vec<PromptRecord>> {
        with_database_read_lock_retry(|| self.list_prompts_once())
    }

    pub(in crate::storage) fn list_prompts_once(&self) -> Result<Vec<PromptRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, tags_json, body, created_at, updated_at
             FROM prompts
             ORDER BY updated_at DESC, title ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let tags_json = row.get::<_, String>(2)?;
            Ok(PromptRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                tags: parse_prompt_tags(&tags_json)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn save_prompt(&self, prompt: PromptWrite) -> Result<PromptRecord> {
        self.with_named_write_transaction("save_prompt", |tx| {
            let now = unix_now().to_string();
            let id = prompt
                .id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(new_prompt_id);
            let title = prompt.title.trim().to_string();
            if title.is_empty() {
                anyhow::bail!("prompt title is required");
            }
            let tags = normalize_prompt_tags(prompt.tags);
            let category = tags.first().cloned().unwrap_or_default();
            let tags_json = serde_json::to_string(&tags)?;
            let body = prompt.body.trim_end().to_string();
            let created_at = tx
                .query_row(
                    "SELECT created_at FROM prompts WHERE id = ?1",
                    params![id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .unwrap_or_else(|| now.clone());

            tx.execute(
                "INSERT INTO prompts (id, title, category, tags_json, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                category = excluded.category,
                tags_json = excluded.tags_json,
                body = excluded.body,
                updated_at = excluded.updated_at",
                params![id, title, category, tags_json, body, created_at, now],
            )?;

            Ok(PromptRecord {
                id,
                title,
                tags,
                body,
                created_at,
                updated_at: now,
            })
        })
    }

    pub fn delete_prompts(&self, ids: &[String]) -> Result<usize> {
        let deleted = self.with_named_write_transaction("delete_prompts", |tx| {
            let mut deleted = 0;
            for id in ids {
                deleted += tx.execute("DELETE FROM prompts WHERE id = ?1", params![id])?;
            }
            Ok(deleted)
        })?;
        Ok(deleted)
    }
}
