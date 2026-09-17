# Tendi authority matrix

这张表是 P0 的边界清单。任何新 writer 必须先归属到表中的 owner；没有 owner 的写入不允许新增。

| Domain | 外部 source | Provider/解析 owner | Canonical authority | Projection/read owner | Scope | Revision/event |
| --- | --- | --- | --- | --- | --- | --- |
| sessions | Codex/Claude/Cursor transcript、index、metadata | 对应 provider adapter | `scoped_sessions.data_json` | daemon snapshot + desktop store | `ScopeKey(workspace)` | `projection_heads(sessions)` + `sessions://scan` |
| session-skill links | transcript evidence 与 skill 关系 | session-skill indexer | `scoped_session_skill_index`、`scoped_session_skill_links` | daemon linked-session commands | `ScopeKey(workspace)` | session-skill index status |
| transcript page | transcript 文件 | 对应 provider transcript parser | 文件 source version + page cursor | daemon RPC response | `SourceLocator` | source version，不写 SQLite |
| skills | skill 文件、frontmatter、git source | skill/provider owner | `normalized_snapshots(scope_key, 'skills')` + `scoped_skill_sources` / `scoped_skill_snapshots` | daemon skill projection | installation/provider/project scope | operation journal + mutation result |
| rules | provider rule files | provider rule scanner | `normalized_snapshots(scope_key, 'rules')` + source/manifest tables | daemon rules list | provider/project scope | projection refresh state |
| hooks | provider config files | provider hook scanner/writer | `normalized_snapshots(scope_key, 'hooks')` + source/manifest tables | daemon hooks list | provider/source path | stale hash conflict |
| MCP | provider config files | provider MCP scanner/writer | `normalized_snapshots(scope_key, 'mcp')` + source/manifest tables | daemon MCP list | provider/project scope | stale source conflict |
| analytics | session transcript source | analytics parser/capability owner | `scoped_session_analytics` + `scoped_session_analytics_overview` | daemon overview query | `ScopeKey(workspace)` + session source identity | scoped projection head |
| settings | local app settings | storage normalization | `app_settings` | daemon settings command | installation | operation journal |
| events | committed projection mutation | daemon coordinator | operation journal + projection head | desktop reducer | event `scopeKey` | `baseRevision -> revision` |

## Writer map

| Writer | Allowed boundary | Must not do |
| --- | --- | --- |
| daemon RPC | prepare 声明资源 -> 非阻塞准入 -> workload continuation -> Store API -> shared writer 短事务 -> event | 全局 mutation 锁、占用执行 worker 等文件锁、自行加数据库锁、暴露原始可写连接 |
| session watcher | 领域任务 -> session rows/revision/index pending 同事务 | 覆盖全量 JSON snapshot、把索引失败报告成 metadata 提交失败 |
| analytics worker | 事务外解析 -> shared writer 短批次 | 在数据库事务内解析 transcript 或等待另一个业务任务 |
| Tauri embedded daemon | 同 daemon RPC boundary | 页面直接调用 Store |
| standalone CLI | daemon RPC attach；无 daemon 时 scoped Store API + 同一 writer 实现 | 调用方自行加锁或重放含文件副作用的操作 |
| desktop store | snapshot/reducer action | 页面自己维护业务 truth |
| provider parser | normalize source record | 推断另一个 provider 的字段语义 |

执行池按负载隔离，不拥有业务排他权。文件排他权归实际资源路径；同一 provider 配置文件跨 hooks/MCP/config、跨 workspace 共享 lease。SQL 原子性归数据库事务，派生投影发布归 scope/domain revision CAS。Skill projection refresh 不修复文件；链接和 wrapper 修复归显式 Skill maintenance。

资源准入和执行是独立阶段，reservation 整体取得后才交给 worker；阶段 continuation 不重放副作用。投影失效与维护任务保存在逐资源 generation receipt 中，按物理 manifest owner 定位受影响的 workspace；确认旧 receipt 不得清除新变更。搜索由 provider-owned JSONL checkpoint 与逐 session 持久化任务驱动，CLI 与 daemon 共用同一发布/CAS 规则。

## Required fields

每个异步 domain mutation 至少要能关联：

```text
scope_key, operation_id, input_revision, output_revision,
source_version, parser_version, status, error
```

失败状态必须是显式终态。文件多步更新要有 staging/rollback；SQLite 多行更新必须在一个 transaction 内提交；event 只能在 commit 成功后发送。

## Current migration boundary

- 数据库和 workspace transition 统一由 `crates/tendi-core/src/migrations/` 管理，作为 storage 内部模块接入统一 writer；`Store::open` 只在 writer 事务内确保 schema 可用，进程启动调用 `tendi_core::initialize_workspace` 完成数据库数据和 workspace transition。需要远程解析 abbreviated Git revision 的维护命令保留显式入口，Git 准备不占数据库写事务。
- session 已使用 scoped projection 和 revisioned snapshot。
- CLI session/catalog list 与 search 优先 attach 同 workspace daemon；无 daemon 时使用自行管理短事务的 scoped Store。SQLite 负责跨进程写互斥，资源文件 lease 只用于业务协调。
- analytics 已使用带复合 scope key 的物理缓存，不再从全局 analytics 表做 workspace 内存过滤。
- scoped session search 使用已发布的同 scope FTS；metadata 提交同时写 pending generation。索引工作独立恢复、分批提交，每次发布携带该事务的准确 revision；后到的新 generation 不被旧任务清除。
- skill mutation 已禁止 `names` 作为定位符；调用必须使用稳定 ID，read-only 展示仍可携带 display name。unknown-provider 只允许显式的 shared-format 归一化，不得成为 provider fallback。
