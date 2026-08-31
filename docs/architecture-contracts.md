# Tendi 运行时架构契约

这份文档把 transcript 中反复出现的故障归并为可验证的系统约束，并记录当前实现边界。

## Top 5 故障簇 + 横切性能故障

| 故障簇 | 根因 | 架构保证 |
| --- | --- | --- |
| transcript 解析错位 | 不同 provider 共用猜测式 parser；tool result 没有可靠 call id | provider 自己声明 parser；未知格式只产生 warning；tool result 只按精确 call id 绑定 |
| session 串 workspace / stale row | SQLite canonical row、缓存和前端列表各自写入；scope 只存在于内存 | workspace session projection 使用 `scoped_sessions(scope_key, id, agent, path)`；`data_json` 是 row authority |
| session-skill 关系串 workspace | session skill index 读写旧的全局表 | index 与双向关联使用 `scoped_session_skill_index`、`scoped_session_skill_links`，查询和清理都带 `ScopeKey` |
| 扫描与 watcher 并发覆盖 | recent、backfill、watch、analytics 各自起线程写库；事件没有 revision | daemon 内单一 `OperationCoordinator` 串行化 mutation；事件携带 operation、base revision、revision、scope |
| skill 误更新 / 半应用 | 用 name 作为 mutation identity；多文件写入中途失败 | 前端优先传 `skillIds`；重复 display name 必须显式消歧；文件 apply 失败自动回滚，数据库 source/snapshot 同一事务提交 |
| 大列表跳转和分页错乱 | viewport / dataset 变化后复用旧 range 和旧 locator | virtual range 每次按当前 count clamp；locator 等待 mounted window；snapshot resync 替代重复全量 list |

## 不变量

1. **单一写入口**：所有 daemon mutation 进入 `OperationCoordinator`。读操作可以直接执行，不因扫描排队。
2. **单一 canonical authority**：session 的完整对象在 `data_json`；标量列只用于索引和兼容读取。
3. **scope 先于 identity**：session identity 是 `scope_key + provider/agent + native id + path`；同一个 native id 在不同 provider 或 workspace 不得合并。
4. **事件必须可判断**：revisioned event 的 `baseRevision` 必须等于本地 revision 才能应用；旧事件丢弃，出现 gap 立即请求 snapshot。
5. **全量 snapshot 是替换语义**：snapshot 只接受服务端给出的 revision 和 rows，不再在 backfill 结束后额外调用旧的全量 session list 接口。带 warning 的 domain 只写 failed 状态，不覆盖最近一次成功 snapshot。
6. **mutation 可回滚**：filesystem 多文件 apply 和 SQLite persistence 都必须满足 all-or-nothing；失败不得留下半套 skill。skill source version 在提交时做 compare-and-swap，旧 preview 不能覆盖新版本。

## 数据流

```text
provider parser
    -> canonical SessionRecord / SkillRecord
    -> OperationCoordinator
    -> SQLite canonical projection + projection_heads
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

CLI 的 scan、skill、backup、session/catalog list 与 search 优先 attach 同 workspace daemon；无 daemon 时才通过 OS database-write lock 使用 scoped Store。daemon 路径进入同一个 coordinator。

## 迁移和删除顺序

1. 所有 desktop session 读取切换到 `sessions_snapshot` 和 scoped list。
2. 所有 desktop mutation 保持在 coordinator 中，并记录 `operation_journal`。
3. provider parser 完成显式覆盖后，删除 auto-import 的多 parser 探测入口。
4. skill clients 全部切换到 `skillIds` 后，删除 names-only mutation API。
5. scoped search 只使用同 scope 的可重建 FTS 派生索引；daemon 进程第一次扫描时清空该 scope 的旧索引，并从 canonical `scoped_sessions` 全量重建，后续 session 写入在同一 SQLite transaction 内更新索引。

## 数据库启动策略

- 旧的安装级 session、skill 和 search 业务投影不做数据迁移；新版本只从 provider source 在首次 scoped scan/refresh 时重建 canonical projection。
- 首次 session scan 可以删除并重建 `scoped_session_search_*` 这类派生索引。它不删除 `scoped_sessions.data_json`、`normalized_snapshots`、`app_settings`、项目映射或 skill source version。
- `Store::open` 仍保留必要的结构兼容检查（例如补列和索引），因为这是保证已安装版本能打开数据库的 schema contract，不是把旧业务数据搬到新 projection。
- 不在每次构造 Store 时删整库：这样会丢失用户设置、项目别名、source CAS 状态和未完成 operation journal。若产品明确接受这些数据全部丢失，才另行提供显式 reset 命令。

## 验证入口

- Rust：`cargo test -p tendi-core scoped_session_projection_keeps_workspaces_isolated`
- Rust：`cargo test -p tendi-core runtime_contract operation_journal projection_heads`
- Desktop：`npm run typecheck`
- Desktop：`node --experimental-strip-types --test scripts/runtime-contract.test.ts scripts/desktop-store.test.ts scripts/data-table-virtualization.test.ts`
