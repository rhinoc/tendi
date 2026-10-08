//! Shared cache values and source identities, with workspace projections kept separate.
use super::{compress_analytics_json, decompress_analytics_json};
use anyhow::{Context, Result};
use rusqlite::{Connection, Params, functions::FunctionFlags, types::ValueRef};
use sha2::{Digest, Sha256};

pub(super) fn register_functions(conn: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function("tendi_cache_hash", 1, flags, |ctx| {
        let bytes = match ctx.get_raw(0) {
            ValueRef::Null => return Ok(None),
            ValueRef::Text(value) | ValueRef::Blob(value) => value,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(
                    "cache value must be text or bytes".into(),
                ));
            }
        };
        Ok(Some(Sha256::digest(bytes).to_vec()))
    })?;
    conn.create_scalar_function("tendi_cache_compress", 1, flags, |ctx| {
        compress_analytics_json(&ctx.get::<String>(0)?).map_err(function_error)
    })?;
    conn.create_scalar_function("tendi_cache_decompress", 1, flags, |ctx| {
        decompress_analytics_json(&ctx.get::<Vec<u8>>(0)?).map_err(function_error)
    })?;
    conn.create_scalar_function("tendi_analytics_text", 1, flags, |ctx| {
        match ctx.get_raw(0) {
            ValueRef::Text(value) => {
                String::from_utf8(value.to_vec()).map_err(|error| function_error(error.into()))
            }
            ValueRef::Blob(value) => decompress_analytics_json(value).map_err(function_error),
            _ => Err(rusqlite::Error::InvalidParameterName(
                "analytics value must be text or encoded bytes".into(),
            )),
        }
    })?;
    conn.create_scalar_function("tendi_parser_version", 1, flags, |ctx| {
        Ok(crate::analytics::parser_state_version(
            &ctx.get::<String>(0)?,
        ))
    })?;
    Ok(())
}

fn function_error(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(format!("{error:#}"))))
}

/// SQLite does not include INSTEAD OF trigger changes in `changes()`. Cache writes
/// use the connection's actual changes to retain their existing no-op semantics.
pub(super) fn execute_changed(
    conn: &Connection,
    sql: &str,
    params: impl Params,
) -> rusqlite::Result<usize> {
    let before = conn.total_changes();
    conn.execute(sql, params)?;
    Ok(usize::from(conn.total_changes() != before))
}

pub(super) fn installed(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='shared_cache_values')",
        [], |row| row.get(0),
    )?)
}

#[derive(Clone, Copy)]
enum Codec {
    Text,
    Json,
    Analytics,
    Overview,
}

struct Layout {
    table: &'static str,
    session_id: &'static str,
    session_path: &'static str,
    payloads: &'static [(&'static str, Codec)],
    ignore_existing: bool,
}

const LAYOUTS: &[Layout] = &[
    Layout {
        table: "scoped_sessions",
        session_id: "id",
        session_path: "path",
        payloads: &[("data_json", Codec::Text)],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_search_entries",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_search_index",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[
            ("search_metadata", Codec::Text),
            ("search_checkpoint", Codec::Json),
        ],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_analytics",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[
            ("analytics_json", Codec::Analytics),
            ("parser_state_json", Codec::Json),
        ],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_analytics_overview",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[("overview_json", Codec::Overview)],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_skill_links",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[("evidence_text", Codec::Text)],
        ignore_existing: true,
    },
    Layout {
        table: "scoped_session_skill_index",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[],
        ignore_existing: false,
    },
    Layout {
        table: "scoped_session_scan_sources",
        session_id: "session_id",
        session_path: "session_path",
        payloads: &[],
        ignore_existing: false,
    },
];

struct Column {
    name: String,
    declared: String,
    required: bool,
    default: Option<String>,
    key: i64,
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<Column>> {
    let mut query = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(query
        .query_map([], |row| {
            Ok(Column {
                name: row.get(1)?,
                declared: row.get(2)?,
                required: row.get::<_, i64>(3)? != 0,
                default: row.get(4)?,
                key: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

impl Layout {
    fn physical(&self, name: &str) -> String {
        if name == "scope_key" {
            "scope_ref".into()
        } else if name == self.session_id || name == "agent" || name == self.session_path {
            "session_ref".into()
        } else if name == "source_path" {
            "source_ref".into()
        } else if self.payloads.iter().any(|(field, _)| *field == name) {
            format!("{name}_ref")
        } else {
            name.into()
        }
    }

    fn plain(&self, field: &str, codec: Codec) -> String {
        let source = if matches!(codec, Codec::Analytics) {
            format!("tendi_analytics_text(NEW.{field})")
        } else {
            format!("NEW.{field}")
        };
        if matches!(codec, Codec::Analytics | Codec::Overview) {
            format!(
                "CASE WHEN json_valid({source}) THEN json_remove({source}, '$.project') ELSE {source} END"
            )
        } else {
            source
        }
    }

    fn reference(&self, name: &str) -> String {
        if name == "scope_key" {
            "(SELECT id FROM cache_scopes WHERE scope_key=NEW.scope_key)".into()
        } else if name == self.session_id || name == "agent" || name == self.session_path {
            format!(
                "(SELECT id FROM cache_sessions WHERE session_id=NEW.{} AND agent=NEW.agent AND session_path=NEW.{})",
                self.session_id, self.session_path
            )
        } else if name == "source_path" {
            "(SELECT id FROM cache_source_paths WHERE path=NEW.source_path)".into()
        } else if let Some((_, codec)) = self.payloads.iter().find(|(field, _)| *field == name) {
            format!(
                "(SELECT id FROM shared_cache_values WHERE kind='{name}' AND digest=tendi_cache_hash({}))",
                self.plain(name, *codec)
            )
        } else {
            format!("NEW.{name}")
        }
    }
}

pub(super) fn install(conn: &Connection) -> Result<()> {
    if installed(conn)? {
        return Ok(());
    }
    let has_rows: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM scoped_sessions UNION ALL SELECT 1 FROM scoped_session_analytics UNION ALL SELECT 1 FROM scoped_session_search_entries UNION ALL SELECT 1 FROM scoped_session_skill_links)", [], |row| row.get(0))?;
    conn.execute_batch(
        "CREATE TABLE cache_scopes(id INTEGER PRIMARY KEY, scope_key TEXT NOT NULL UNIQUE);
         CREATE TABLE cache_sessions(id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, agent TEXT NOT NULL, session_path TEXT NOT NULL, UNIQUE(session_id,agent,session_path));
         CREATE TABLE cache_source_paths(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
         CREATE TABLE shared_cache_values(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, digest BLOB NOT NULL, value NOT NULL, ref_count INTEGER NOT NULL DEFAULT 0 CHECK(ref_count>=0), UNIQUE(kind,digest));
         DROP TRIGGER IF EXISTS scoped_session_search_entries_ai;
         DROP TRIGGER IF EXISTS scoped_session_search_entries_ad;
         DROP TRIGGER IF EXISTS scoped_session_search_entries_au;
         INSERT INTO scoped_session_search_metadata_fts(scoped_session_search_metadata_fts) VALUES('delete-all');"
    )?;
    for layout in LAYOUTS {
        install_layout(conn, layout)
            .with_context(|| format!("normalize {} cache", layout.table))?;
    }
    install_search_triggers(conn)?;
    conn.execute_batch(
        "INSERT INTO scoped_session_search_metadata_fts(rowid,metadata_text,title,project)
         SELECT rowid,metadata_text,title,project FROM scoped_session_search_entries_storage
         WHERE metadata_text<>'' OR title<>'' OR project<>'';
         DELETE FROM session_search_content_records WHERE NOT EXISTS(
             SELECT 1 FROM scoped_session_search_entries_storage e WHERE e.content_id=session_search_content_records.id);
"
    )?;
    if has_rows {
        conn.execute("INSERT INTO meta(key,value) VALUES('storage.compaction.pending','1') ON CONFLICT(key) DO UPDATE SET value='1'", [])?;
    }
    Ok(())
}

fn install_layout(conn: &Connection, layout: &Layout) -> Result<()> {
    let name = layout.table;
    let storage = format!("{name}_storage");
    let legacy = format!("{name}_unshared");
    let cols = columns(conn, name)?;
    let indexes = {
        let mut query = conn.prepare(
            "SELECT name FROM sqlite_schema WHERE type='index' AND tbl_name=?1 AND sql IS NOT NULL",
        )?;
        query
            .query_map([name], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut index_sql = Vec::new();
    for index in indexes {
        let mut query = conn.prepare(&format!("PRAGMA index_xinfo({index})"))?;
        let fields = query
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut keys = Vec::new();
        for (field, descending, key) in fields {
            if key == 0 {
                continue;
            }
            if let Some(field) = field {
                let field = layout.physical(&field);
                if !keys.iter().any(|(name, _)| *name == field) {
                    keys.push((field, descending));
                }
            }
        }
        index_sql.push(format!(
            "DROP INDEX {index}; CREATE INDEX {index} ON {storage}({});",
            keys.iter()
                .map(|(field, descending)| format!(
                    "{field}{}",
                    if *descending != 0 { " DESC" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    conn.execute_batch(&format!("ALTER TABLE {name} RENAME TO {legacy};"))?;
    let mut definitions = Vec::new();
    let mut physical_cols = Vec::new();
    let mut values = Vec::new();
    for col in &cols {
        let field = layout.physical(&col.name);
        if physical_cols.contains(&field) {
            continue;
        }
        let replaced = field != col.name;
        let declared = if replaced {
            "INTEGER"
        } else {
            col.declared.as_str()
        };
        let default = if replaced {
            String::new()
        } else {
            col.default
                .as_ref()
                .map(|value| format!(" DEFAULT {value}"))
                .unwrap_or_default()
        };
        let primary_id = col.name == "id" && col.name != layout.session_id;
        definitions.push(format!(
            "{field} {declared}{}{}{}",
            if primary_id {
                " PRIMARY KEY"
            } else if col.required {
                " NOT NULL"
            } else {
                ""
            },
            default,
            if primary_id { " AUTOINCREMENT" } else { "" }
        ));
        let value = layout.reference(&col.name);
        values.push(if col.required && !replaced {
            col.default
                .as_ref()
                .map(|default| format!("COALESCE({value},{default})"))
                .unwrap_or(value)
        } else {
            value
        });
        physical_cols.push(field);
    }
    let overlay = layout
        .payloads
        .iter()
        .any(|(_, codec)| matches!(codec, Codec::Analytics | Codec::Overview));
    if overlay {
        definitions.push("project_overlay TEXT".into());
        physical_cols.push("project_overlay".into());
        let (field, codec) = layout
            .payloads
            .iter()
            .find(|(_, codec)| matches!(codec, Codec::Analytics | Codec::Overview))
            .unwrap();
        let input = if matches!(codec, Codec::Analytics) {
            format!("tendi_analytics_text(NEW.{field})")
        } else {
            format!("NEW.{field}")
        };
        values.push(format!(
            "CASE WHEN json_valid({input}) THEN {input} -> '$.project' END"
        ));
    }
    if name == "scoped_session_analytics" {
        definitions.push("parser_version INTEGER NOT NULL DEFAULT 0".into());
        physical_cols.push("parser_version".into());
        values.push("tendi_parser_version(NEW.parser_state_json)".into());
    }
    let mut key_cols = cols.iter().filter(|col| col.key > 0).collect::<Vec<_>>();
    key_cols.sort_by_key(|col| col.key);
    let mut key = Vec::new();
    for col in key_cols {
        let field = layout.physical(&col.name);
        if !key.contains(&field) {
            key.push(field);
        }
    }
    // Search entries have a row identity plus a source ordinal uniqueness rule.
    if name == "scoped_session_search_entries" {
        key = vec![
            "scope_ref".into(),
            "session_ref".into(),
            "record_order".into(),
        ];
    }
    if !key.is_empty() {
        definitions.push(format!("UNIQUE({})", key.join(",")));
    }
    conn.execute_batch(&format!(
        "CREATE TABLE {storage}({}); {}",
        definitions.join(","),
        index_sql.join("\n")
    ))?;
    create_projection(conn, layout, &cols, &physical_cols, &values, &key)?;
    migrate_rows(conn, layout, &legacy, &storage, &physical_cols, &values)?;
    Ok(())
}

fn create_projection(
    conn: &Connection,
    layout: &Layout,
    cols: &[Column],
    physical_cols: &[String],
    values: &[String],
    key: &[String],
) -> Result<()> {
    let name = layout.table;
    let storage = format!("{name}_storage");
    let mut select = vec!["r.rowid AS rowid".to_string()];
    let mut joins = format!(
        "JOIN cache_scopes sc ON sc.id=r.scope_ref JOIN cache_sessions ss ON ss.id=r.session_ref"
    );
    for col in cols {
        let expression = if col.name == "scope_key" {
            "sc.scope_key".into()
        } else if col.name == layout.session_id {
            "ss.session_id".into()
        } else if col.name == "agent" {
            "ss.agent".into()
        } else if col.name == layout.session_path {
            "ss.session_path".into()
        } else if col.name == "source_path" {
            joins.push_str(" JOIN cache_source_paths sp ON sp.id=r.source_ref");
            "sp.path".into()
        } else if let Some((_, codec)) =
            layout.payloads.iter().find(|(field, _)| *field == col.name)
        {
            let alias = format!("p_{}", col.name);
            joins.push_str(&format!(
                " LEFT JOIN shared_cache_values {alias} ON {alias}.id=r.{}_ref",
                col.name
            ));
            match codec {
                Codec::Text => format!("CAST({alias}.value AS TEXT)"),
                Codec::Json => format!(
                    "CASE WHEN {alias}.id IS NOT NULL THEN tendi_cache_decompress({alias}.value) END"
                ),
                Codec::Analytics | Codec::Overview => {
                    let plain = format!("tendi_cache_decompress({alias}.value)");
                    let restored = format!(
                        "CASE WHEN r.project_overlay IS NOT NULL AND json_valid({plain}) THEN json_set({plain},'$.project',json(r.project_overlay)) ELSE {plain} END"
                    );
                    if matches!(codec, Codec::Analytics) {
                        select.push(format!("{restored} AS analytics_text"));
                        format!("tendi_cache_compress({restored})")
                    } else {
                        restored
                    }
                }
            }
        } else {
            format!("r.{}", col.name)
        };
        select.push(format!("{expression} AS {}", col.name));
    }
    if name == "scoped_session_analytics" {
        select.push("r.parser_version".into());
    }
    conn.execute_batch(&format!(
        "CREATE VIEW {name} AS SELECT {} FROM {storage} r {joins};",
        select.join(",")
    ))?;
    let mut intern = format!(
        "INSERT OR IGNORE INTO cache_scopes(scope_key) VALUES(NEW.scope_key);
         INSERT OR IGNORE INTO cache_sessions(session_id,agent,session_path) VALUES(NEW.{},NEW.agent,NEW.{});", layout.session_id, layout.session_path
    );
    if cols.iter().any(|col| col.name == "source_path") {
        intern.push_str("INSERT OR IGNORE INTO cache_source_paths(path) VALUES(NEW.source_path);");
    }
    for (field, codec) in layout.payloads {
        let plain = layout.plain(field, *codec);
        let encoded = if matches!(codec, Codec::Text) {
            plain.clone()
        } else {
            format!("tendi_cache_compress({plain})")
        };
        intern.push_str(&format!("INSERT OR IGNORE INTO shared_cache_values(kind,digest,value) SELECT '{field}',tendi_cache_hash({plain}),{encoded} WHERE NEW.{field} IS NOT NULL AND NOT EXISTS(SELECT 1 FROM shared_cache_values WHERE kind='{field}' AND digest=tendi_cache_hash({plain}));"));
    }
    let updates = physical_cols
        .iter()
        .filter(|field| !(*field == "id" && layout.session_id != "id"))
        .collect::<Vec<_>>();
    let assignments = updates
        .iter()
        .map(|field| format!("{field}=excluded.{field}"))
        .collect::<Vec<_>>()
        .join(",");
    let changed = updates
        .iter()
        .map(|field| format!("{storage}.{field} IS NOT excluded.{field}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let conflict = if layout.ignore_existing {
        "DO NOTHING".into()
    } else {
        format!("DO UPDATE SET {assignments} WHERE {changed}")
    };
    let value_list = values.join(",");
    let insert_fields = format!("rowid,{}", physical_cols.join(","));
    let insert_values = format!("NEW.rowid,{value_list}");
    let set = physical_cols
        .iter()
        .zip(values)
        .map(|(field, value)| format!("{field}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    let update_changed = physical_cols
        .iter()
        .zip(values)
        .map(|(field, value)| format!("{field} IS NOT {value}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let new_refs = layout
        .payloads
        .iter()
        .map(|(field, _)| format!("COALESCE(NEW.{field}_ref,0)"))
        .collect::<Vec<_>>()
        .join(",");
    let old_refs = layout
        .payloads
        .iter()
        .map(|(field, _)| format!("COALESCE(OLD.{field}_ref,0)"))
        .collect::<Vec<_>>()
        .join(",");
    if !new_refs.is_empty() {
        conn.execute_batch(&format!(
            "CREATE TRIGGER {storage}_retain AFTER INSERT ON {storage} BEGIN UPDATE shared_cache_values SET ref_count=ref_count+1 WHERE id IN({new_refs}); END;
             CREATE TRIGGER {storage}_release AFTER DELETE ON {storage} BEGIN UPDATE shared_cache_values SET ref_count=ref_count-1 WHERE id IN({old_refs}); DELETE FROM shared_cache_values WHERE ref_count=0 AND id IN({old_refs}); END;
             CREATE TRIGGER {storage}_replace AFTER UPDATE ON {storage} BEGIN
               UPDATE shared_cache_values SET ref_count=ref_count+1 WHERE id IN({new_refs}) AND id NOT IN({old_refs});
               UPDATE shared_cache_values SET ref_count=ref_count-1 WHERE id IN({old_refs}) AND id NOT IN({new_refs});
               DELETE FROM shared_cache_values WHERE ref_count=0 AND id IN({old_refs}); END;"
        ))?;
    }
    let cleanup = if layout.payloads.is_empty() {
        String::new()
    } else {
        format!(
            "DELETE FROM shared_cache_values WHERE ref_count=0 AND id IN({});",
            layout
                .payloads
                .iter()
                .map(|(field, _)| layout.reference(field))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    conn.execute_batch(&format!(
        "CREATE TRIGGER {name}_insert INSTEAD OF INSERT ON {name} BEGIN {intern} INSERT INTO {storage}({insert_fields}) VALUES({insert_values}) ON CONFLICT({}) {conflict}; {cleanup} END;
         CREATE TRIGGER {name}_update INSTEAD OF UPDATE ON {name} BEGIN {intern} UPDATE {storage} SET {set} WHERE rowid=OLD.rowid AND ({update_changed}); {cleanup} END;
         CREATE TRIGGER {name}_delete INSTEAD OF DELETE ON {name} BEGIN DELETE FROM {storage} WHERE rowid=OLD.rowid; END;
         ", key.join(",")
    ))?;
    Ok(())
}

pub(super) fn refresh_projections(conn: &Connection) -> Result<()> {
    for layout in LAYOUTS {
        let storage = format!("{}_storage", layout.table);
        let physical = columns(conn, &storage)?;
        let mut logical = columns(conn, layout.table)?;
        logical.retain(|column| {
            column.name != "rowid"
                && !(layout.table == "scoped_session_analytics"
                    && matches!(column.name.as_str(), "parser_version" | "analytics_text"))
        });
        for column in &mut logical {
            if let Some(stored) = physical
                .iter()
                .find(|stored| stored.name == layout.physical(&column.name))
            {
                column.required = stored.required;
                column.default = stored.default.clone();
            }
        }
        let fields = physical
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let values = fields
            .iter()
            .map(|field| {
                if field == "project_overlay" {
                    let (payload, codec) = layout
                        .payloads
                        .iter()
                        .find(|(_, codec)| matches!(codec, Codec::Analytics | Codec::Overview))
                        .unwrap();
                    let input = if matches!(codec, Codec::Analytics) {
                        format!("tendi_analytics_text(NEW.{payload})")
                    } else {
                        format!("NEW.{payload}")
                    };
                    format!("CASE WHEN json_valid({input}) THEN {input} -> '$.project' END")
                } else if field == "parser_version" && layout.table == "scoped_session_analytics" {
                    "tendi_parser_version(NEW.parser_state_json)".into()
                } else {
                    let column = logical
                        .iter()
                        .find(|column| layout.physical(&column.name) == *field)
                        .unwrap();
                    let value = layout.reference(&column.name);
                    if column.required && layout.physical(&column.name) == column.name {
                        column
                            .default
                            .as_ref()
                            .map(|default| format!("COALESCE({value},{default})"))
                            .unwrap_or(value)
                    } else {
                        value
                    }
                }
            })
            .collect::<Vec<_>>();
        let key_query = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name=?1",
            [&storage],
            |row| row.get::<_, String>(0),
        )?;
        let key = key_query
            .rsplit_once("UNIQUE(")
            .context("cache membership uniqueness missing")?
            .1
            .split(')')
            .next()
            .unwrap()
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        conn.execute_batch(&format!(
            "DROP VIEW {}; DROP TRIGGER IF EXISTS {storage}_retain;
             DROP TRIGGER IF EXISTS {storage}_release; DROP TRIGGER IF EXISTS {storage}_replace;",
            layout.table,
        ))?;
        create_projection(conn, layout, &logical, &fields, &values, &key)?;
    }
    Ok(())
}

fn migrate_rows(
    conn: &Connection,
    layout: &Layout,
    source: &str,
    destination: &str,
    fields: &[String],
    values: &[String],
) -> Result<()> {
    conn.execute_batch(&format!(
        "INSERT OR IGNORE INTO cache_scopes(scope_key) SELECT DISTINCT scope_key FROM {source};
         INSERT OR IGNORE INTO cache_sessions(session_id,agent,session_path)
           SELECT DISTINCT {id},agent,{path} FROM {source};",
        id = layout.session_id,
        path = layout.session_path,
    ))?;
    if fields.iter().any(|field| field == "source_ref") {
        conn.execute_batch(&format!("INSERT OR IGNORE INTO cache_source_paths(path) SELECT DISTINCT source_path FROM {source};"))?;
    }
    let mut select = values
        .iter()
        .map(|value| value.replace("NEW.", "source."))
        .collect::<Vec<_>>();
    let mut joins = String::new();
    let mut stages = Vec::new();
    for (field, codec) in layout.payloads {
        let stage = format!("{source}_{field}_values");
        let plain = layout.plain(field, *codec).replace("NEW.", "source.");
        conn.execute_batch(&format!(
            "CREATE TEMP TABLE {stage} AS WITH payloads AS MATERIALIZED (
                 SELECT rowid AS source_rowid,{plain} AS plain FROM {source} source WHERE {field} IS NOT NULL
             ) SELECT source_rowid,tendi_cache_hash(plain) AS digest,plain FROM payloads;
             CREATE UNIQUE INDEX {stage}_row ON {stage}(source_rowid);"
        ))?;
        let encoded = if matches!(codec, Codec::Text) {
            "plain"
        } else {
            "tendi_cache_compress(plain)"
        };
        conn.execute_batch(&format!(
            "INSERT OR IGNORE INTO shared_cache_values(kind,digest,value)
             SELECT '{field}',digest,{encoded} FROM {stage}
             WHERE NOT EXISTS(SELECT 1 FROM shared_cache_values v WHERE v.kind='{field}' AND v.digest={stage}.digest)
             GROUP BY digest;"
        ))?;
        joins.push_str(&format!(
            " LEFT JOIN {stage} stage_{field} ON stage_{field}.source_rowid=source.rowid"
        ));
        let position = fields
            .iter()
            .position(|column| *column == format!("{field}_ref"))
            .unwrap();
        select[position] = format!(
            "(SELECT id FROM shared_cache_values WHERE kind='{field}' AND digest=stage_{field}.digest)"
        );
        stages.push(stage);
    }
    conn.execute_batch(&format!(
        "INSERT INTO {destination}(rowid,{}) SELECT source.rowid,{} FROM {source} source {joins}; DROP TABLE {source};",
        fields.join(","), select.join(","),
    ))?;
    for stage in stages {
        conn.execute_batch(&format!("DROP TABLE {stage};"))?;
    }
    Ok(())
}

fn install_search_triggers(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TRIGGER scoped_session_search_entries_ai AFTER INSERT ON scoped_session_search_entries_storage
         WHEN NEW.metadata_text<>'' OR NEW.title<>'' OR NEW.project<>'' BEGIN
           INSERT INTO scoped_session_search_metadata_fts(rowid,metadata_text,title,project) VALUES(NEW.rowid,NEW.metadata_text,NEW.title,NEW.project); END;
         CREATE TRIGGER scoped_session_search_entries_ad AFTER DELETE ON scoped_session_search_entries_storage BEGIN
           DELETE FROM scoped_session_search_metadata_fts WHERE rowid=OLD.rowid AND (OLD.metadata_text<>'' OR OLD.title<>'' OR OLD.project<>'');
           DELETE FROM session_search_content_records WHERE id=OLD.content_id AND NOT EXISTS(SELECT 1 FROM scoped_session_search_entries_storage WHERE content_id=OLD.content_id); END;
         CREATE TRIGGER scoped_session_search_entries_au AFTER UPDATE ON scoped_session_search_entries_storage BEGIN
           DELETE FROM scoped_session_search_metadata_fts WHERE rowid=OLD.rowid AND (OLD.metadata_text<>'' OR OLD.title<>'' OR OLD.project<>'');
           INSERT INTO scoped_session_search_metadata_fts(rowid,metadata_text,title,project) SELECT NEW.rowid,NEW.metadata_text,NEW.title,NEW.project WHERE NEW.metadata_text<>'' OR NEW.title<>'' OR NEW.project<>'';
           DELETE FROM session_search_content_records WHERE id=OLD.content_id AND OLD.content_id<>NEW.content_id AND NOT EXISTS(SELECT 1 FROM scoped_session_search_entries_storage WHERE content_id=OLD.content_id); END;"
    )?;
    Ok(())
}

pub(super) fn rebuild_skill_links(conn: &Connection) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT sql FROM sqlite_schema WHERE tbl_name IN
         ('scoped_session_skill_links_storage','scoped_session_skill_links') AND sql IS NOT NULL
         ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'view' THEN 2 ELSE 3 END",
    )?;
    let definitions = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    conn.execute_batch(
        "PRAGMA writable_schema=ON;
         DELETE FROM sqlite_schema WHERE tbl_name IN
         ('scoped_session_skill_links_storage','scoped_session_skill_links');
         PRAGMA writable_schema=OFF; VACUUM;",
    )?;
    for definition in definitions {
        conn.execute_batch(&definition)?;
    }
    conn.execute_batch(
        "UPDATE shared_cache_values SET ref_count=(
             SELECT count(*) FROM scoped_session_skill_links_storage WHERE evidence_text_ref=shared_cache_values.id
         ) WHERE kind='evidence_text';
         DELETE FROM shared_cache_values WHERE kind='evidence_text' AND ref_count=0;
         DELETE FROM scoped_session_skill_index;",
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "shared_cache_tests.rs"]
mod tests;
