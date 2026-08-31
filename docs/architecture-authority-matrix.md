# Tendi authority matrix

这张表是 P0 的边界清单。任何新 writer 必须先归属到表中的 owner；没有 owner 的写入不允许新增。

| Domain | 外部 source | Provider/解析 owner | Canonical authority | Projection/read owner | Scope | Revision/event |
| --- | --- | --- | --- | --- | --- | --- |
| sessions | Codex/Claude/Cursor transcript、index、metadata | 对应 provider adapter | `scoped_sessions.data_json` | daemon snapshot + desktop store | `ScopeKey(workspace)` | `projection_heads(sessions)` + `sessions://scan` |
| session-skill links | transcript evidence 与 skill 关系 | session-skill indexer | `scoped_session_skill_index`、`scoped_session_skill_links` | daemon linked-session commands | `ScopeKey(workspace)` | session-skill index status |
| transcript page | transcript 文件 | 对应 provider transcript parser | 文件 source version + page cursor | daemon RPC response | `SourceLocator` | source version，不写 SQLite |
| skills | skill 文件、frontmatter、git source | skill/provider owner | `normalized_snapshots(scope_key, 'skills')` + source/snapshot tables | daemon skill projection | installation/provider/project scope | operation journal + mutation result |
| rules | provider rule files | provider rule scanner | `normalized_snapshots(scope_key, 'rules')` + source/manifest tables | daemon rules list | provider/project scope | projection refresh state |
| hooks | provider config files | provider hook scanner/writer | `normalized_snapshots(scope_key, 'hooks')` + source/manifest tables | daemon hooks list | provider/source path | stale hash conflict |
| MCP | provider config files | provider MCP scanner/writer | `normalized_snapshots(scope_key, 'mcp')` + source/manifest tables | daemon MCP list | provider/project scope | stale source conflict |
| analytics | session transcript source | analytics parser/capability owner | `scoped_session_analytics` + `scoped_session_analytics_overview` | daemon overview query | `ScopeKey(workspace)` + session source identity | scoped projection head |
| settings | local app settings | storage normalization | `app_settings` | daemon settings command | installation | operation journal |
| events | committed projection mutation | daemon coordinator | operation journal + projection head | desktop reducer | event `scopeKey` | `baseRevision -> revision` |

## Writer map

| Writer | Allowed boundary | Must not do |
| --- | --- | --- |
| daemon RPC | `OperationCoordinator` -> short SQLite transaction -> event | 直接在 request thread 写 SQLite |
| session watcher | coordinator job -> scoped delta transaction | 直接覆盖全量 session snapshot |
| analytics worker | coordinator job -> analytics transaction | 与 session scan 并行持有 writer |
| Tauri embedded daemon | 同 daemon RPC boundary | 页面直接调用 Store |
| standalone CLI | daemon RPC attach；无 daemon 时 database-write lock + scoped Store API | 写 legacy global session projection |
| desktop store | snapshot/reducer action | 页面自己维护业务 truth |
| provider parser | normalize source record | 推断另一个 provider 的字段语义 |

## Required fields

每个异步 domain mutation 至少要能关联：

```text
scope_key, operation_id, input_revision, output_revision,
source_version, parser_version, status, error
```

失败状态必须是显式终态。文件多步更新要有 staging/rollback；SQLite 多行更新必须在一个 transaction 内提交；event 只能在 commit 成功后发送。

## Current migration boundary

- session 已使用 scoped projection 和 revisioned snapshot。
- CLI session/catalog list 与 search 优先 attach 同 workspace daemon；无 daemon 时才使用带 OS lock 的 scoped Store 路径。
- analytics 已使用带复合 scope key 的物理缓存，不再从全局 analytics 表做 workspace 内存过滤。
- scoped session search 当前读取 canonical metadata，不能访问 legacy 全局 FTS；建立 scoped FTS 后再切换。
- skill mutation 已禁止 `names` 作为定位符；调用必须使用稳定 ID，read-only 展示仍可携带 display name。unknown-provider 只允许显式的 shared-format 归一化，不得成为 provider fallback。
