# MCP 与 Hooks Tab 信息架构和数据来源梳理

## 范围

本文件对应任务 `task-73529a227f737b0dd852703da4344e0c`，目标是为后续增强 Hooks 详情页和 MCP tab 核心能力提供文件清单、当前能力判断和最小改动方案。

## 入口与加载链路

- 导航入口在 `apps/desktop/src/lib/helpers.ts` 的 `navItems` 中注册，`hooks` 与 `mcp` 都是一级 sidebar tab。
- 全局状态在 `apps/desktop/src/App.tsx` 里按 agent 过滤，其中 `hooks` 和 `mcp` 直接过滤原始行数据。
- 首次进入 tab 时，`apps/desktop/src/App.tsx` 会通过 `${domain}_list` 懒加载数据；因此 Hooks 触发 `hooks_list`，MCP 触发 `mcp_list`。
- Hooks 使用专门的 `HooksView`，MCP 使用 `DataListView`，当前 MCP 行操作由 `apps/desktop/src/views/McpView.tsx` 提供。
- `normalizeReport` 仅展开 `report.hooks.hooks` 与 `report.mcp.servers`，没有针对 Hooks/MCP 的字段规范化或错误元数据保留。
- `safeInvoke` 在失败时只记录 warning 并返回 `null`，没有把错误带回 tab 状态，见当前 `apps/desktop/src/lib/tauri.ts`。

## MCP Tab 现状

### 前端组件

- 文件入口：`apps/desktop/src/views/McpView.tsx`。
- 当前组件名仍是通用的 `DataListView`，负责标题、选择状态、`DataTable` 渲染，以及 MCP 启用状态开关，见 `apps/desktop/src/views/McpView.tsx`。
- 行 ID 由 `id ?? agent:name:path` 拼出，包含来源路径，见 `apps/desktop/src/views/McpView.tsx`。
- 选择能力已打开，底部工具栏支持批量 Enable/Disable、Reveal in Finder 和复制路径。
- 表格列包括 Agent、Name、Transport、Enabled、Status、Source；Enabled 列使用现成 `Switch`。
- 空状态为 `No MCP servers found`，加载态和失败态由 `DataTable`、`LoadErrorState` 处理。

### 后端数据

- Tauri 命令包括 `mcp_list` 和 `mcp_set_enabled`；列表返回 `tendi_core::mcp::scan_mcp(&cwd).servers`，启用命令写回配置后重新扫描。
- `McpServerRecord` 包含 `agent`、`name`、`scope`、`transport`、`status`、`path`、`server_path`、`trust_hash`、`read_only_reason`；`server_path` 用于定位 Claude `~/.claude.json` 中按项目嵌套的 `mcpServers`。
- 扫描来源包括：
  - `~/.codex/config.toml`
  - `~/.claude/settings.json`
  - `~/.claude.json` 顶层个人 MCP，以及其中 `projects[项目路径].mcpServers`
  - `~/.cursor/cli-config.json`
  - `~/.cursor/projects`
  - 当前工作目录 ancestors 下的 `.mcp.json`、`.cursor/mcp.json`、`.codex/mcp.json`、`.codex/config.toml`
  - 对应实现见 `crates/tendi-core/src/mcp.rs:29-85`。
- 配置键、文件名、transport/status 推断和写回字段由对应 provider 提供：Codex 在 `providers/codex.rs` 同时处理 TOML/JSON，Claude Code 在 `providers/claude.rs` 处理 JSON，Cursor 在 `providers/cursor.rs` 处理 JSON；`crates/tendi-core/src/mcp.rs` 只提供 JSON/TOML 格式工具。
- 当前状态是静态推断：`disabled` 或 `enabled=false` 变成 `disabled`，否则是 `configured`；Cursor plugin metadata 可根据 `STATUS.md` 标记 `needs-auth`。标准 JSON/TOML 配置记录源文件 hash，用于启用状态写回时拒绝覆盖外部修改；Cursor plugin metadata 标记为只读。

### MCP 当前能力边界

结论：MCP 已具备启用/禁用最小闭环；详情、健康检查和认证等能力仍未实现。

- 已补齐最小后端接口：Tauri 现在有 `mcp_list` 和 `mcp_set_enabled`。后者先按 `agent` 分派到 Codex、Claude Code、Cursor provider，再由 provider 选择允许的配置格式；底层校验源文件 hash。仍没有 delete、health check、auth flow、tool/resource/list detail 等命令。
- 缺数据：`McpServerRecord` 不包含 command、args、env、url、headers、tool/resource counts、last probe result、auth hint、配置 scope、可编辑性等信息。
- UI 已接线：表格增加 Enabled 开关和选中行的批量 Enable/Disable；仍没有 detail panel。
- 状态反馈不足：warnings 保留在 `McpScan`，但 `mcp_list` 只返回 servers，前端拿不到 parse/read warnings；`safeInvoke` 也吞掉错误。

## Hooks Tab 现状

### 前端组件

- 文件入口：`apps/desktop/src/views/HooksView.tsx`。
- 当前 IA 是左右分栏：左侧列表、右侧详情，支持搜索、选择、行点击切换详情、详情折叠。
- 列表字段：Event、Agent、Type、Matcher、Enabled，定义在 `apps/desktop/src/views/HooksView.tsx`。
- 搜索使用 `hookSearchText`，覆盖 agent、event、matcher、filter、status message、type、handler、path/source、trust hash，见 `apps/desktop/src/lib/hooks.ts`。
- 详情操作：Reveal in Finder、Delete hook、Collapse，见 `apps/desktop/src/views/HooksView.tsx`。
- 详情字段：
  - Match: Event、Matcher、If
  - Handler: Type、Command、URL、Prompt
  - Source: Path、Trust
  - 对应实现见 `apps/desktop/src/views/HooksView.tsx`。
- Delete/Enable 可用性由 `HookRecord.read_only_reason` 提供；provider 负责判断托管源，provider 负责选择 JSON/TOML 写回格式，前端不再按扩展名推断能力。

### 后端数据

- Tauri 命令包括 `hooks_list` 与 `hook_delete`，见 `apps/desktop/src-tauri/src/lib.rs:269-310`。
- `HookRecord` 字段包括 `agent`、`event`、`matcher`、`hook_type`、`command`、`url`、`prompt`、`filter`、`status_message`、`enabled`、`path`、`trust_hash`、`needs_review`，见 `crates/tendi-core/src/hooks.rs:14-28`。
- 扫描来源包括：
  - Codex: `$CODEX_HOME/hooks.json`、`$CODEX_HOME/config.toml`、项目 ancestors 下 `.codex/hooks.json`、`.codex/config.toml`
  - Cursor: `~/.cursor/hooks.json`、`/etc/cursor/hooks.json`、`/Library/Application Support/Cursor/hooks.json`、项目 ancestors 下 `.cursor/hooks.json`
  - Claude: `~/.claude/settings.json`、`.claude/settings(.local).json`、`.claude/plugins`、`.claude/skills`、`.claude/agents`、系统 managed settings
- 对应实现见 `crates/tendi-core/src/providers/{codex,claude,cursor}.rs`，`hooks.rs` 只负责扫描编排、hash 校验和格式工具。
- `needs_review` 对 Codex 映射官方 hook trust 状态；Cursor 和 Claude 使用 Tendi 的源码 hash 审批记录（`~/Library/Application Support/tendi/hook-reviews.json`），托管和插件来源不要求 review。Claude 的 `disableAllHooks` 会反映到 `enabled`。
- 删除和 Enable/Disable 前会校验当前文件 sha256 是否等于 `trust_hash`；请求携带 `agent`，由 provider 决定允许的源格式和只读语义。

### Hooks 详情页已具备和缺失能力

已具备：

- 能查看核心 match/handler/source 字段。
- 能复制命令、URL、Prompt、Path、Trust。
- 能 reveal 源文件。
- 对可删除的 json/toml hook 能执行删除，并在删除后重新扫描 hooks。
- 对系统或 plugin hook 有禁用态说明。

主要缺口：

- 没有编辑能力：不能修改 event、matcher、enabled、filter、status、handler。
- 没有创建或 duplicate 能力。
- 没有 raw source preview，用户无法查看 hook 在原文件中的完整上下文。
- 没有诊断能力：不能校验命令是否存在、URL 是否可达、prompt 是否为空、matcher 是否命中常见工具。
- 没有 hook 运行历史或最近触发记录。当前数据源也没有这类数据。
- 错误态不完整：`hook_delete` 失败只显示固定文案，真实错误只在 console；`hooks_list` 失败没有 tab 级错误。
- 空状态区分不足：`No matching hooks` 同时覆盖无 hook、搜索无结果、加载失败等场景。

## 现有测试覆盖

- `apps/desktop/scripts/smoke-render.mjs` 只验证 SSR 包含 tab 文案。
- `apps/desktop/scripts/alignment-e2e.mjs` 把 Hooks/MCP 作为统一表格布局检查目标。
- `apps/desktop/scripts/smoke-ui.mjs:1738-1760` 验证 Hooks 可渲染详情、可折叠，MCP 可渲染表格并选择行。
- 当前没有覆盖 MCP detail/action，也没有覆盖 Hooks 编辑、删除成功/失败细节、列表错误态或空状态分支。

## 最小改动方案

### 1. MCP tab 核心能力

- 将 `DataListView` 拆成专用 `McpView`，保留 `DataTable` 列表，但增加右侧 detail panel。
- 后端当前已为 `McpServerRecord` 增加 `trust_hash` 和 `read_only_reason`，用于启用状态写回和只读提示；`id`、`command`、`args`、`url`、`env_keys` 等详情字段仍未增加。启用/禁用由各 provider 的 `set_mcp_enabled` 实现负责分派。
- Tauri 增加最小命令：
  - `mcp_list` 返回 `{ servers, warnings }` 或新增 `mcp_scan`，前端能显示 warnings。
  - `mcp_reveal_source(path)` 可复用现有 `reveal_in_finder`，前端直接接线即可。
  - `mcp_set_enabled(path, name, enabled, expectedHash)` 已实现，当前只支持可编辑 JSON/TOML 的 enable/disable。
- UI 首批操作入口：
  - Reveal source
  - Copy command/url
  - Enable/disable when editable
  - Show read-only reason when system/plugin/unsupported
  - Warnings/needs-auth banner
- 后续再做真实 server probe、tool/resource discovery，因为这需要启动或连接 MCP server，风险和耗时高于本期最小闭环。

### 2. Hooks detail 增强

- 保留现有分栏和 `HookDetailRow`，新增 raw/source preview 区，直接读取源文件或返回目标 hook 的 source snippet。
- 增加 edit flow：先支持 enabled、matcher、filter、status、handler 字段编辑；保存时沿用 trust hash 防并发覆盖。
- 后端新增 `hook_update`，复用 `delete_hook` 的匹配逻辑和 trust hash 校验，按 json/toml 更新匹配 hook。
- 删除错误直接返回到 `HooksView`，展示真实错误文案；`App.deleteHook` 不再把所有错误压成 `null`。
- 空状态拆分：
  - loading: Loading hooks
  - search empty: No hooks match this search
  - true empty: No hooks configured
  - error: Could not load hooks, with retry

### 3. 共用状态与验证

- 增加 domain-level `{ rows, loading, error, warnings }` 或最小增加 `domainErrors`，不要继续让 `safeInvoke` 吞掉 tab 级错误。
- `mcp_list` 和 `hooks_list` 保留 scan warnings，前端在 tab header 或 detail panel 显示。
- 更新 `apps/desktop/scripts/smoke-ui.mjs`：
  - MCP: 点击行显示 detail，copy/reveal/disabled action 可见，warnings 可见。
  - Hooks: 删除失败显示真实错误；只读 hook 显示原因；空状态和搜索空状态分开。
- 更新 Rust 单元测试：
  - MCP 记录包含新增字段和 warnings。
  - Hook update 对 json/toml、生效的 trust hash、只读路径分别测试。

## 建议实现顺序

1. 先改 MCP 数据模型和 Tauri 返回值，让 UI 有足够信息显示详情和不可用原因。
2. 再实现 `McpView` 专用详情页和基础操作，保持列表列定义可复用。
3. 然后增强 Hooks 错误态和 raw preview，这是低风险增量。
4. 最后做 `hook_update` 和 Hooks 编辑 UI，因为它涉及源文件结构化写回，测试成本最高。
