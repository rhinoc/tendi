//! assistant persistence through the database-owned transaction boundary.
use super::super::*;

impl Store {
    pub fn list_assistant_chat_sessions(&self) -> Result<Vec<AssistantChatSession>> {
        with_database_read_lock_retry(|| self.list_assistant_chat_sessions_once())
    }

    pub(in crate::storage) fn list_assistant_chat_sessions_once(
        &self,
    ) -> Result<Vec<AssistantChatSession>> {
        let session_rows = {
            let mut statement = self.conn.prepare(
                "SELECT id, linked_session_id, linked_session_agent, linked_session_path
                 FROM assistant_chat_sessions
                 ORDER BY updated_at DESC, id ASC",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        session_rows
            .into_iter()
            .map(|(id, linked_id, linked_agent, linked_path)| {
                let linked_session = match (linked_id, linked_agent, linked_path) {
                    (Some(id), Some(agent), Some(path)) => {
                        Some(AssistantSessionLink { id, agent, path })
                    }
                    (None, None, None) => None,
                    _ => bail!("assistant chat session has an incomplete linked session"),
                };
                let mut statement = self.conn.prepare(
                    "SELECT role, content
                     FROM assistant_chat_messages
                     WHERE session_id = ?1
                     ORDER BY id ASC",
                )?;
                let messages = statement
                    .query_map([&id], |row| {
                        Ok(AssistantMessage {
                            role: row.get(0)?,
                            content: row.get(1)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(AssistantChatSession {
                    id,
                    messages,
                    linked_session,
                })
            })
            .collect()
    }

    pub fn append_assistant_chat_message(
        &self,
        conversation_id: &str,
        workspace: &Path,
        linked_session: Option<&AssistantSessionLink>,
        message: &AssistantMessage,
    ) -> Result<()> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            bail!("assistant conversation id is required")
        }
        if !matches!(message.role.as_str(), "user" | "assistant") {
            bail!("assistant message role is invalid")
        }
        let content = message.content.trim();
        if content.is_empty() {
            bail!("assistant message content is required")
        }
        let workspace = workspace.to_string_lossy().trim().to_string();
        if workspace.is_empty() {
            bail!("assistant workspace is required")
        }
        let now = Local::now().to_rfc3339();
        let (linked_id, linked_agent, linked_path) = linked_session
            .map(|session| {
                (
                    Some(session.id.as_str()),
                    Some(session.agent.as_str()),
                    Some(session.path.as_str()),
                )
            })
            .unwrap_or((None, None, None));
        self.with_named_write_transaction("append_assistant_chat_message", |tx| {
            tx.execute(
                "INSERT INTO assistant_chat_sessions (
                id, workspace, linked_session_id, linked_session_agent, linked_session_path,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
                params![
                    conversation_id,
                    workspace,
                    linked_id,
                    linked_agent,
                    linked_path,
                    now,
                ],
            )?;
            tx.execute(
                "INSERT INTO assistant_chat_messages (session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![conversation_id, message.role, content, now],
            )?;
            Ok(())
        })?;
        Ok(())
    }
}
