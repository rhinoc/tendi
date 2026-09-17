# Tendi 运行时架构契约

这份文档把 transcript 中反复出现的故障归并为可验证的系统约束，并记录当前实现边界。

## Top 5 故障簇 + 横切性能故障

| 故障簇 | 根因 | 架构保证 |
| --- | --- | --- |
| transcript 解析错位 | 不同 provider 共用猜测式 parser；tool result 没有可靠 call id | provider 自己声明 parser；未知格式只产生 warning；tool result 只按精确 call id 绑定 |
| session 串 workspace / stale row | SQLite canonical row、缓存和前端列表各自写入；scope 只存在于内存 | workspace session projection 使用 `scoped_sessions(scope_key, id, agent, path)`；`data_json` 是 row authority |
| session-skill 关系串 workspace | session skill index 读写旧的全局表 | index 与双向关联使用 `scoped_session_skill_index`、`scoped_session_skill_links`，查询和清理都带 `ScopeKey` |
| 扫描与 watcher 并发覆盖 | recent、backfill、watch、analytics 的业务顺序与数据库写入顺序混淆；事件没有 revision | 业务 coordinator 保持领域操作顺序；每个数据库的共享 writer 串行执行短事务；事件携带 operation、base revision、revision、scope |
| skill 误更新 / 半应用 | 用 name 作为 mutation identity；多文件写入中途失败 | 前端优先传 `skillIds`；重复 display name 必须显式消歧；文件 apply 失败自动回滚，数据库 source/snapshot 同一事务提交 |
| 大列表跳转和分页错乱 | viewport / dataset 变化后复用旧 range 和旧 locator | virtual range 每次按当前 count clamp；locator 等待 mounted window；snapshot resync 替代重复全量 list |

## 不变量

1. **单一数据库写入口**：所有生产持久化方法自行进入共享 writer，包括 CLI、daemon、索引和迁移。调用方不加数据库锁。业务 coordinator 负责操作顺序与任务生命周期，不持有数据库写连接。读操作使用独立连接，不进入写队列。
2. **单一 canonical authority**：session 的完整对象在 `data_json`；标量列只用于索引和兼容读取。
3. **scope 先于 identity**：session identity 是 `scope_key + provider/agent + native id + path`；同一个 native id 在不同 provider 或 workspace 不得合并。
4. **事件必须可判断**：revisioned event 的 `baseRevision` 必须等于本地 revision 才能应用；旧事件丢弃，出现 gap 立即请求 snapshot。
5. **全量 snapshot 是替换语义**：snapshot 只接受服务端给出的 revision 和 rows，不再在 backfill 结束后额外调用旧的全量 session list 接口。带 warning 的 domain 只写 failed 状态，不覆盖最近一次成功 snapshot。
6. **mutation 可回滚**：SQLite 原子组在同一事务提交；filesystem 多文件更新通过 staging/rollback 补偿失败，不声称文件系统与 SQLite 是一个原子事务。skill source version 在提交时做 compare-and-swap，旧 preview 不能覆盖新版本。资源锁按实际文件或安装目录协调，不替代版本检查。
7. **索引有独立生命周期**：canonical session 与索引待处理标记同事务提交。搜索解析、差量比较在写事务外执行，索引按短批次写入、完成后发布；索引失败保留待处理状态和错误，不把已提交的 session 更新报告成失败。

agents、skills、rules、hooks、MCP 的投影 list 请求读取 `normalized_snapshots` 的最近一次快照，并通过 `projection_status` 判断是否需要刷新。缺失或过期时，请求立即返回已有快照（首次为空），刷新由按 scope/domain 去重的后台任务执行；完成后发出 `projection://changed`。session 不写全量 JSON 快照，直接读取 canonical rows，列表和 revision 在同一个短读事务内取得。写操作仍保留同步投影读取，因为 mutation 必须基于最新快照做冲突检查。

## 存储机制与锁顺序

- `storage.rs` 提供存储入口；领域 SQL 位于 `storage/repositories/`，连接、写队列和事务机制分别由 `database.rs`、`writer.rs`、`transaction.rs` 持有。
- 每个 canonical 数据库路径在进程内共享一个 writer 和一个可写连接。持久化事务固定为 `BEGIN IMMEDIATE`；跨 repository 的原子操作传递同一个事务，不递归开启新事务。
- 写队列最多容纳 256 个等待事务，准入预算 30 秒。交互与后台各自 FIFO；后台在等待时最多连续执行 3 个交互事务，随后让出一个后台名额。搜索索引批次使用后台优先级；这是进程内调度，不保证跨进程公平。过期请求在执行前撤销，已经执行的请求返回实际提交或回滚结果，不先报超时再后台提交。业务回调执行一次，不因锁竞争重放文件修改。
- 扫描、Git、文件读写、解析和大块序列化在数据库事务外准备；提交时重新检查受并发影响的 source version。业务资源锁由 `coordination::ResourceLease` 管理，与数据库队列无关。
- 允许的持有顺序是文件资源 lease → 投影构建 lease（需要时）→ writer 排队 → SQLite 事务。投影构建不得反向修改安装文件；事务内不得等待业务队列、重新获取资源 lease 或提交另一个数据库事务。文件与对应元数据提交在同一资源 lease 生命周期内，释放后再刷新派生投影。
- 日志分别记录操作名、数据库、队列等待、SQLite 等待和事务耗时，避免把排队时间误判为 SQL 执行时间。
- mutation RPC 不再经过全局 `operations` 队列或 control/skill authority 锁。`RequestScheduler` 为交互、外部 I/O、计算、准备分别分配 2/4/2/2 个 worker；每个原始请求类别最多 128 个在途请求，后续阶段不改变其准入名额归属。轻量查询和取消不入这些队列；任务池限制负载，资源 owner 与事务校验负责正确性。
- 每个执行阶段先声明资源，独立 admission pump 只在有执行名额时整体 `try-acquire`。冲突请求留在待准入队列，不占执行 worker；较早的冲突资源请求阻止后来的冲突请求插队，无关资源继续推进。等待预算累计 30 秒，不包含已开始阶段的执行时间；每个 `FnOnce` continuation 只执行一次，取消、超时和退出丢弃尚未开始的阶段，不重放网络或文件副作用。
- 搜索 worker 只处理所属 daemon scope，事件不广播其他 workspace 的 revision；退出在会话和短批次边界检查取消并保留 pending。已开始的单次文件解析和数据库事务先完成，不声称可强行中断。

## 文件资源与投影边界

- 文件互斥使用安装级统一 namespace，与 workspace、领域和数据库路径无关。hooks、MCP、config 编辑同一 provider 文件时共享同一个资源。普通读取不获取这些写 lease；数据库读使用独立只读连接，并非给业务领域套 `RwLock`。
- 路径同时保护 symlink 入口和 canonical 目标。资源目录与其子路径冲突，兄弟文件可并行；内部祖先共享意向锁仅用来表达写资源层级，不是读侧锁。
- 多资源先规范化、排序、去重，再整体尝试获取；一项冲突即释放已取得的部分，不持有 A 等待 B。等待总预算 30 秒。准入取得的 reservation 可转交执行线程，再进入不可跨线程移动的 lease；同线程只允许复用外层已经覆盖的资源集合，禁止嵌套扩张。Git 写入同时声明逻辑 `.git`、实际 git-dir 和 common-dir，linked worktree 共享的 refs/objects 不绕过协调。
- provider 负责声明配置资源和文件语义，core 写入口持有 lease，CLI 与 daemon 使用同一套机制。跨文件操作仍是 staging/rollback 补偿，不是文件系统与 SQLite 的联合事务。
- Skill projection refresh 只读取安装文件并发布投影，不修复链接或 wrapper；Skill maintenance 才负责这些文件变更，按 scope 合并为独立后台工作。扫描补齐默认 visibility 使用初始化语义并重读胜出值，不覆盖并发的用户选择。
- 五个快照领域使用 revision CAS 发布；失效和 canonical skill 元数据变更在短事务内推进 revision 并记录持久化 dirty receipt，旧扫描不能覆盖新变更。明确的资源变更允许基于最近快照只刷新受影响的 skill 安装和依赖；无精确来源、缺失快照或版本迁移才执行全量扫描。CLI 聚合扫描在扫描前捕获六域版本，在同一个事务全部校验后发布，失败只重试纯扫描。
- `projection_dirty_resources` 分别保存投影和文件维护 generation；成功发布/维护仅确认捕获版本之前的任务，新变更保留。物理资源 owner 来自扫描时解析的 `fs_manifest.resource_path` 和实际安装根目录；失效通过索引查找引用该资源的 workspace，不广播全部 workspace。纯投影刷新不清除尚未执行的维护任务。
- 持久化 dirty 保证已经提交的任务跨重启保留，不提供文件系统与 SQLite 的跨存储原子性：文件写入成功、dirty 事务提交之前进程崩溃，仍需要后续源扫描发现变化。严格的文件 intent/recovery 协议尚未实现，不能把短事务或资源 lease 等同于跨存储 crash-safe commit。
- 设置按字段 patch，在一个事务内更新并返回一致快照；profile 有独立原子命令。项目扫描提交前重新校验 scope 配置。preview 按 ID 独立保存、一次消费，容量 64、有效期 30 分钟，不再互相覆盖单个槽位。

## 增量索引与实现边界

- `scoped_session_search_work` 按完整 session identity 持久化任务与 generation，canonical 更新与任务入队同事务。worker 只读取待处理键；删除也持久化为同 identity 的清理任务，不扫描整个 scope 来推断本轮变化。
- Codex、Claude、Cursor 各自声明 JSONL parser/checkpoint 版本和追加语义。checkpoint 记录文件身份、已提交字节偏移、行/record 顺序及有限前缀和边界校验。普通追加只读新增尾部与固定大小校验区；不完整 JSON/UTF-8 尾部留待下次，合法无换行 EOF 记录可临时索引，但不越过可重读的 checkpoint。
- 替换、截断、同长度改写、parser 版本变化触发重建。增量契约要求 provider 不改写已提交历史；同 inode 同时任意改写中段并追加，不能通过尾读和有限校验完整检测，已知这种改写的调用方必须显式 rebuild。
- 搜索提交按行数、字节数和约 10ms SQL 工作预算切批；预算在语句之间检查，单条语句与 commit 不受硬截止约束。资源公平性是单个 daemon 的准入保证，不承诺跨进程 FIFO；跨进程由同一资源 namespace 和 SQLite 保证互斥。

## 数据流

```text
provider parser
    -> canonical SessionRecord / SkillRecord
    -> 领域操作协调 / 准备变更
    -> shared writer: canonical projection + projection_heads + index pending
    -> revisioned DaemonEvent
    -> desktop store reducer
    -> virtualized view
```

事件元数据统一使用 camelCase：

```json
{
  "id": 42,
  "event": "sessions://scan",
  "scopeKey": "workspace:/repo",
  "domain": "sessions",
  "operationId": "session-scan-7",
  "baseRevision": 12,
  "revision": 13,
  "payload": {}
}
```

## Provider 和 identity

provider trait 负责识别、路径、parser、状态和 source locator。共享层只负责调度和格式化，不依据文件扩展名推断 provider。session 对外暴露稳定的 `SessionKey` / `SourceLocator`，文件移动不会改变 native identity；不同 provider 的同名 native id 仍然隔离。

 skill mutation 的落地入口统一是 `skillIds`；文件编辑入口统一是 `skillId`。CLI 的 pattern 只负责在当前 projection 解析成 IDs，不能直接作为写入定位符。display name 只用于展示、搜索和新 wrapper 的目标名称，不参与既有安装的定位。

Skill identity follows the installation boundary: one record represents one canonical filesystem directory, and its discovered paths are aliases only when they canonicalize to that same directory. Independent copies are separate records regardless of name, source metadata, or content hash. Any ambiguous name reference is rejected or omitted; it never selects the first record.

CLI 的 scan、skill、backup、session/catalog list 与 search 优先 attach 同 workspace daemon；无 daemon 时使用同一套 scoped Store API 和进程内 writer。跨进程事务互斥交给 SQLite 的 BEGIN IMMEDIATE；不再维护额外的 database-write 文件锁。daemon 的业务队列与数据库写队列是两个不同边界。

## 迁移和删除顺序

1. 所有 desktop session 读取切换到 `sessions_snapshot` 和 scoped list。
2. 所有 desktop mutation 保持在 coordinator 中，并记录 `operation_journal`。
3. provider parser 完成显式覆盖后，删除 auto-import 的多 parser 探测入口。
4. skill clients 全部切换到 `skillIds` 后，删除 names-only mutation API。
5. scoped search 只使用同 scope 的可重建 FTS 派生索引。session 事务写入逐 identity 的 `scoped_session_search_work`，scope pending 保留为聚合/显式重建标记；索引任务校验 source version，分批更新并发布。generation 在处理期间增加时，旧任务不得清除新请求；重启后继续处理持久化任务，不清空全部索引。

## 数据库启动策略

- 旧的安装级 session、skill 和 search 业务投影不做数据迁移；新版本只从 provider source 在首次 scoped scan/refresh 时重建 canonical projection。
- [v0.1.3](https://github.com/rhinoc/tendi/releases/tag/v0.1.3) 及此前正式版使用未版本化数据库（`user_version = 0`），没有 workspace-scoped 投影。未发布的 scoped schema 已压成一份当前定义，pending/work/dirty 表和 checkpoint/resource-path 列直接创建，不保留开发版逐级迁移或补任务逻辑。
- 当前数据库 schema 直接以 scoped projection 为唯一模型：旧的全局投影表、重复的 per-field 表和旧的 session/search 表不再创建，也不做未发布过程态的兼容迁移。新 scoped 投影由正常扫描建立。
- `Store::open` 在共享 writer 的事务内检查并初始化 schema；取得事务后再次检查版本，避免并发启动重复迁移。数据和文件 transition 仍由启动入口显式执行，不在每次读连接打开时重复执行。
- 不在每次构造 Store 时删整库：这样会丢失用户设置、项目别名、source CAS 状态和未完成 operation journal。若产品明确接受这些数据全部丢失，才另行提供显式 reset 命令。

## 验证入口

- Rust：`cargo test -p tendi-core scoped_session_projection_keeps_workspaces_isolated`
- Rust：`cargo test -p tendi-core runtime_contract operation_journal projection_heads`
- Desktop：`npm run typecheck`
- Desktop：`node --experimental-strip-types --test scripts/runtime-contract.test.ts scripts/desktop-store.test.ts scripts/data-table-virtualization.test.ts`
