# MCP 与 Hooks Tab 信息架构和数据来源梳理

## 范围

本文件对应任务 `task-73529a227f737b0dd852703da4344e0c`，目标是为后续增强 Hooks 详情页和 MCP tab 核心能力提供文件清单、当前能力判断和最小改动方案。

## 入口与加载链路

- 导航入口在 `apps/desktop/src/lib/helpers.jsx:96-103` 的 `navItems` 中注册，`hooks` 与 `mcp` 都是一级 sidebar tab。
- 全局状态在 `apps/desktop/src/App.jsx:118-128` 里按 agent 过滤，其中 `hooks` 和 `mcp` 直接过滤原始行数据。
- 首次进入 tab 时，`apps/desktop/src/App.jsx:236-265` 会通过 `${domain}_list` 懒加载数据；因此 Hooks 触发 `hooks_list`，MCP 触发 `mcp_list`。
- 渲染分发在 `apps/desktop/src/App.jsx:467-470`：Hooks 使用专门的 `HooksView`，MCP 只使用通用 `DataListView`。
- `normalizeReport` 仅展开 `report.hooks.hooks` 与 `report.mcp.servers`，没有针对 Hooks/MCP 的字段规范化或错误元数据保留，见 `apps/desktop/src/lib/helpers.jsx:843-864`。
- `safeInvoke` 在失败时只记录 warning 并返回 `null`，没有把错误带回 tab 状态，见当前 `apps/desktop/src/lib/tauri.ts`。

## MCP Tab 现状

### 前端组件

- 文件入口：`apps/desktop/src/views/McpView.jsx`。
- 当前组件名仍是通用的 `DataListView`，只负责标题、选择状态和 `DataTable` 渲染，见 `apps/desktop/src/views/McpView.jsx:9-43`。
- 行 ID 由 `id ?? agent:name:transport` 拼出，未包含 `path`，同一 agent 下同名同 transport 但不同来源时可能冲突，见 `apps/desktop/src/views/McpView.jsx:11-13`。
- 选择能力已打开，但 `bottomBar={() => null}`，所以选中后没有批量操作，见 `apps/desktop/src/views/McpView.jsx:31-36`。
- 表格列只有 Agent、Name、Transport、Status、Source，定义在 `apps/desktop/src/components/shared.jsx:2064-2086`。
- 空状态是通用文案 `No results`，加载态是 `Loading mcp`，没有 MCP 专属的权限、不可用、解析错误或无配置解释，见 `apps/desktop/src/views/McpView.jsx:37-39`。

### 后端数据

- Tauri 命令只有 `mcp_list`，返回 `tendi_core::mcp::scan_mcp(&cwd).servers`，见 `apps/desktop/src-tauri/src/lib.rs:312-318`。
- `McpServerRecord` 只有 `agent`、`name`、`transport`、`status`、`path`，见 `crates/tendi-core/src/mcp.rs:14-21`。
- 扫描来源包括：
  - `~/.codex/config.toml`
  - `~/.claude/settings.json`
  - `~/.cursor/cli-config.json`
  - `~/.cursor/projects`
  - 当前工作目录 ancestors 下的 `.mcp.json`、`.cursor/mcp.json`、`.codex/mcp.json`、`.codex/config.toml`
  - 对应实现见 `crates/tendi-core/src/mcp.rs:29-85`。
- JSON 配置读取 `mcpServers`、`mcp_servers`、`servers`，TOML 读取 `mcp_servers`，见 `crates/tendi-core/src/mcp.rs:125-201`。
- 当前状态是静态推断：`disabled` 或 `enabled=false` 变成 `disabled`，否则是 `configured`；Cursor plugin metadata 可根据 `STATUS.md` 标记 `needs-auth`。

### 为什么 MCP 可选中但无实际操作能力

结论：主要是缺后端接口和 UI 未接线，数据也不够深。

- 缺后端接口：Tauri 只有 `mcp_list`，没有 reveal、open config、enable/disable、delete、health check、auth flow、tool/resource/list detail 等命令。
- 缺数据：`McpServerRecord` 不包含 command、args、env、url、headers、tool/resource counts、last probe result、auth hint、配置 scope、可编辑性等信息。
- UI 未接线：表格 selectable 已开启但 `bottomBar` 为空，行点击也没有 detail panel 或 action rail。
- 状态反馈不足：warnings 保留在 `McpScan`，但 `mcp_list` 只返回 servers，前端拿不到 parse/read warnings；`safeInvoke` 也吞掉错误。

## Hooks Tab 现状

### 前端组件

- 文件入口：`apps/desktop/src/views/HooksView.jsx`。
- 当前 IA 是左右分栏：左侧列表、右侧详情，支持搜索、选择、行点击切换详情、详情折叠。
- 列表字段：Event、Agent、Type、Matcher、Enabled，定义在 `apps/desktop/src/views/HooksView.jsx:138-202`。
- 搜索使用 `hookSearchText`，覆盖 agent、event、matcher、filter、status message、type、handler、path/source、trust hash，见 `apps/desktop/src/lib/helpers.jsx:1706-1718`。
- 详情操作：Reveal in Finder、Delete hook、Collapse，见 `apps/desktop/src/views/HooksView.jsx:272-289`。
- 详情字段：
  - Match: Event、Matcher、If
  - Handler: Type、Command、URL、Prompt
  - Source: Path、Trust
  - 对应实现见 `apps/desktop/src/views/HooksView.jsx:296-314`。
- Delete 可用性在前端由 `hookDeleteDisabledReason` 判定：系统路径、Claude plugin 路径、非 json/toml 禁止删除，见 `apps/desktop/src/lib/helpers.jsx:1727-1739`。

### 后端数据

- Tauri 命令包括 `hooks_list` 与 `hook_delete`，见 `apps/desktop/src-tauri/src/lib.rs:269-310`。
- `HookRecord` 字段包括 `agent`、`event`、`matcher`、`hook_type`、`command`、`url`、`prompt`、`filter`、`status_message`、`enabled`、`path`、`trust_hash`、`needs_review`，见 `crates/tendi-core/src/hooks.rs:14-28`。
- 扫描来源包括：
  - Codex: `$CODEX_HOME/hooks.json`、`$CODEX_HOME/config.toml`、项目 ancestors 下 `.codex/hooks.json`、`.codex/config.toml`
  - Cursor: `~/.cursor/hooks.json`、`/etc/cursor/hooks.json`、`/Library/Application Support/Cursor/hooks.json`、项目 ancestors 下 `.cursor/hooks.json`
  - Claude: `~/.claude/settings.json`、`.claude/settings(.local).json`、`.claude/plugins`、`.claude/skills`、`.claude/agents`、系统 managed settings
  - 对应实现见 `crates/tendi-core/src/hooks.rs:50-143`。
- `needs_review` 对 Codex 映射官方 hook trust 状态；Cursor 和 Claude 使用 Tendi 的源码 hash 审批记录（`~/Library/Application Support/tendi/hook-reviews.json`），托管和插件来源不要求 review。Claude 的 `disableAllHooks` 会反映到 `enabled`。
- 删除前会校验当前文件 sha256 是否等于 `trust_hash`，然后只支持 json/toml 源，见 `crates/tendi-core/src/hooks.rs:146-180`。

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
- 后端扩展 `McpServerRecord`：至少增加 `id`、`source_kind/scope`、`editable`、`command`、`args`、`url`、`env_keys`、`warning`。避免把敏感 env value 传到 UI。
- Tauri 增加最小命令：
  - `mcp_list` 返回 `{ servers, warnings }` 或新增 `mcp_scan`，前端能显示 warnings。
  - `mcp_reveal_source(path)` 可复用现有 `reveal_in_finder`，前端直接接线即可。
  - `mcp_set_enabled(id/path/name, enabled, expectedHash)` 或先只支持可编辑 json/toml 的 enable/disable。
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
