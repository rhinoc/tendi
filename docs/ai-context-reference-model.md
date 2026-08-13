# AI 上下文引用数据模型与跨页面状态管理

## 范围

本文件对应任务 `task-d8e3c0c12c7cbf2b81835529f66e7edc`，目标是定义 AI 对话助手收集上下文时使用的轻量引用模型。设计重点是让前端跨页面收集 session、skill 等对象时只保存和发送引用 ID，不把完整对象内容塞进对话请求。AI 运行时再通过本机 CLI、skill 或后端接口按引用拉取详情。

## 设计目标

- 引用 item 必须足够轻，适合放进前端 store、草稿和请求 payload。
- 引用 item 必须可解析，后端或 AI skill 能根据 `type`、`id`、`workspaceId` 和 `sourceRoute` 找回完整对象。
- 同一个对话可从不同页面逐步加入引用，切换页面不丢失。
- 支持 session、skill 首批类型，并为 issue、task、artifact、workspace reference 等类型预留扩展点。
- 权限和可见性信息随引用传递，但只表达访问约束，不复制敏感内容。

## 核心数据模型

```ts
type AiContextReferenceType =
  | "session"
  | "skill"
  | "issue"
  | "task"
  | "artifact"
  | "workspace-reference"
  | "rule"
  | "hook"
  | "mcp-server";

type AiContextVisibility = "visible" | "restricted" | "missing" | "unknown";

type AiContextPermission =
  | "read"
  | "read-transcript"
  | "read-files"
  | "execute"
  | "write";

interface AiContextReference {
  version: 1;
  type: AiContextReferenceType;
  id: string;
  displayName: string;
  workspaceId: string;
  sourceRoute: AiContextSourceRoute;
  createdAt: string;
  createdBy?: "user" | "system" | "assistant";
  visibility: AiContextVisibility;
  permissions: AiContextPermission[];
  provider?: string;
  agent?: "codex" | "cursor" | "claude" | "shared" | "unknown";
  scope?: "workspace" | "project" | "global" | "conversation";
  metadata?: Record<string, string | number | boolean | null>;
}

interface AiContextSourceRoute {
  view: string;
  routeParams?: Record<string, string>;
  href?: string;
  label?: string;
}
```

### 字段说明

- `version`: 引用结构版本。首版固定为 `1`，后续升级时允许后端兼容读取旧草稿。
- `type`: 引用类型。首批实现 `session` 与 `skill`，其余类型只定义 contract。
- `id`: 稳定实体 ID。对于 skill，建议使用规范化后的 skill name；如需区分安装目标，可在 `metadata.primaryPath` 或 `metadata.targetAgent` 中补充。
- `displayName`: UI 展示用名称，不参与解析，不作为唯一键。
- `workspaceId`: 当前 workspace ID。跨 workspace 发送时必须显式分组，不能把不同 workspace 的引用静默合并。
- `sourceRoute`: 用户加入引用时所在页面，供 UI 返回来源和后端调试。
- `createdAt`: ISO 8601 时间戳，用于排序和草稿恢复。
- `visibility`: 加入引用时的可见性快照。发送前后端仍需重新校验。
- `permissions`: 当前用户或本机上下文对该引用的允许操作。对 AI 请求默认只使用读权限。
- `provider`/`agent`: 当引用来自某个 agent provider 或扫描来源时保存，例如 `codex` session、`cursor` skill。
- `scope`: 引用生效范围。用于区分全局 skill、项目 skill、当前对话临时附件等。
- `metadata`: 只放非敏感、小体积辅助字段。不得放 transcript、skill markdown 全文、环境变量值、hook 命令输出等内容。

## 首批类型约定

### Session 引用

```ts
interface AiSessionReference extends AiContextReference {
  type: "session";
  id: string;
  agent: "codex" | "cursor" | "claude" | "unknown";
  permissions: ("read" | "read-transcript")[];
  metadata?: {
    path?: string;
    project?: string;
    updatedAt?: string;
    messageCount?: number;
  };
}
```

解析规则：

- 优先按 `(workspaceId, agent, id)` 查找 session。
- 如果 session ID 在 provider 内不唯一，可用 `metadata.path` 辅助定位，但发送给 AI 的主要身份仍是引用 ID。
- AI 需要 transcript 时，通过本机 skill/CLI 读取，而不是由前端提前展开。

### Skill 引用

```ts
interface AiSkillReference extends AiContextReference {
  type: "skill";
  id: string;
  agent?: "codex" | "cursor" | "claude" | "shared" | "unknown";
  permissions: ("read" | "read-files")[];
  metadata?: {
    installTargets?: string;
    primaryPath?: string;
    visibility?: "auto" | "manual" | "off" | "mixed";
    sourceKind?: string;
    updateStatus?: string;
  };
}
```

解析规则：

- `id` 使用 normalized skill name。
- 多安装目标 skill 默认表示同名 skill 聚合记录；如果用户从某个 target 明确加入，可在 `metadata.primaryPath` 和 `agent` 中固定来源。
- AI 需要文件内容时，通过 skill 文件读取能力按 name/path 拉取。

### 预留类型

- `issue`: `id` 为 workspace issue ID，解析到 issue detail。
- `task`: `id` 为 task ID，`metadata.issueId` 必填。
- `artifact`: `id` 为 artifact ID，解析到 artifact metadata 和本地 path。
- `workspace-reference`: `id` 为 reference ID，`metadata.source` 和 `metadata.groupId` 视来源补充。
- `rule`、`hook`、`mcp-server`: 用于后续把配置项加入 AI 上下文，`metadata.path` 可作为辅助定位，但仍需避免传递敏感配置值。

## 去重和排序

引用唯一键：

```ts
function aiContextReferenceKey(ref: AiContextReference): string {
  const target = ref.metadata?.primaryPath || ref.metadata?.issueId || "";
  return [ref.workspaceId, ref.type, ref.agent || "", ref.id, target].join(":");
}
```

规则：

- 同 key 引用只保留一条。
- 重复加入时保留最早 `createdAt`，合并最新 `displayName`、`sourceRoute`、`visibility` 和 `metadata`。
- 默认排序按 `createdAt` 升序，保证用户加入顺序稳定。
- 发送 payload 前按 `workspaceId` 分组，并在组内保持加入顺序。
- UI 可提供手动 reorder，手动排序只存储 reference key 数组，不改引用本体。

## 持久化范围

推荐三层状态：

```ts
interface AiConversationDraft {
  version: 1;
  workspaceId: string;
  conversationId: string;
  references: AiContextReference[];
  referenceOrder: string[];
  promptDraft: string;
  updatedAt: string;
  restorePolicy: "current-conversation" | "workspace-draft" | "none";
}
```

- 当前对话：默认持久化范围。用户关闭 AI 面板再打开时应保留。
- 当前 workspace 草稿：可选。仅当没有 active conversation 时恢复最近草稿。
- 跨 workspace：不自动恢复。不同 workspace 的上下文可能涉及不同权限和文件系统位置。
- 浏览器 local state 可以缓存草稿，但发送前后端必须重新校验引用仍存在且可读。
- 如果引用失效，UI 保留 item 并标记 `missing`，用户可移除或重新解析。

## 前端 Store 草案

```ts
interface AiContextStoreState {
  open: boolean;
  activeConversationId: string | null;
  workspaceId: string;
  referencesByKey: Record<string, AiContextReference>;
  referenceOrder: string[];
  promptDraft: string;
  restoredAt?: string;
}

interface AiContextStoreActions {
  openAssistant(): void;
  closeAssistant(): void;
  startConversation(workspaceId: string): string;
  addReferences(references: AiContextReference[]): void;
  removeReference(key: string): void;
  clearConversationContext(conversationId: string): void;
  reorderReferences(keys: string[]): void;
  updatePromptDraft(value: string): void;
}
```

页面集成方式：

- Sessions 页面选中行后调用 `addReferences(buildSessionReferences(rows))`。
- Skills 页面选中行后调用 `addReferences(buildSkillReferences(rows))`。
- 全局 AI 按钮只负责打开对话面板，不重置已有上下文。
- 页面切换不卸载 store。React 内可先用 app-level state 或轻量 external store；后续需要多窗口同步时再迁移到后端持久化。

## 前后端边界

前端负责：

- 从当前列表行构造轻量引用。
- 去重、排序、草稿恢复和 UI 状态。
- 展示引用的 `displayName`、`type`、`visibility` 和来源页面。

后端负责：

- 发送前校验引用是否仍存在、是否属于当前 workspace、是否可读。
- 把引用解析为 AI provider 可消费的 resolver instructions。
- 屏蔽敏感字段，例如 env value、token、完整 transcript、hook command output。
- 在本机 AI run 中提供引用解析 skill/CLI 能力。

建议 Tauri/IPC 接口：

```ts
interface ValidateAiContextReferencesRequest {
  workspaceId: string;
  references: AiContextReference[];
}

interface ValidateAiContextReferencesResponse {
  references: AiContextReference[];
  diagnostics: AiContextReferenceDiagnostic[];
}

interface AiContextReferenceDiagnostic {
  key: string;
  severity: "info" | "warning" | "error";
  code: "missing" | "forbidden" | "stale" | "unsupported-type";
  message: string;
}
```

## 发送请求 Payload

```ts
interface AiConversationRequest {
  version: 1;
  workspaceId: string;
  conversationId: string;
  provider: "codex" | "cursor-cloud";
  prompt: string;
  context: {
    references: AiContextReference[];
    order: string[];
  };
  options?: {
    stream: boolean;
    allowWrite: boolean;
    openTerminalFallback: boolean;
  };
}
```

示例：

```json
{
  "version": 1,
  "workspaceId": "99074653-d1cd-40e2-b9ff-c24d7b228445",
  "conversationId": "conv-local-001",
  "provider": "codex",
  "prompt": "分析这些 session 使用过的 skill，并建议如何优化。",
  "context": {
    "references": [
      {
        "version": 1,
        "type": "session",
        "id": "fd12d6ce-3e8f-4a25-8489-76fbf76e0e31",
        "displayName": "AI 对话助手设计任务",
        "workspaceId": "99074653-d1cd-40e2-b9ff-c24d7b228445",
        "sourceRoute": { "view": "sessions", "label": "Sessions" },
        "createdAt": "2026-06-25T08:00:00.000Z",
        "visibility": "visible",
        "permissions": ["read", "read-transcript"],
        "agent": "codex",
        "scope": "workspace"
      },
      {
        "version": 1,
        "type": "skill",
        "id": "skill-creator",
        "displayName": "skill-creator",
        "workspaceId": "99074653-d1cd-40e2-b9ff-c24d7b228445",
        "sourceRoute": { "view": "skills", "label": "Skills" },
        "createdAt": "2026-06-25T08:01:00.000Z",
        "visibility": "visible",
        "permissions": ["read", "read-files"],
        "agent": "codex",
        "scope": "global"
      }
    ],
    "order": [
      "99074653-d1cd-40e2-b9ff-c24d7b228445:session:codex:fd12d6ce-3e8f-4a25-8489-76fbf76e0e31:",
      "99074653-d1cd-40e2-b9ff-c24d7b228445:skill:codex:skill-creator:"
    ]
  },
  "options": {
    "stream": true,
    "allowWrite": false,
    "openTerminalFallback": true
  }
}
```

## AI Resolver Contract

发送到本机 AI 的 system/developer 上下文应包含明确约束：

- 你收到的是引用，不是完整内容。
- 需要详情时使用可用 skill/CLI 解析引用。
- 不要猜测不可见或缺失引用的内容。
- 读详情前优先列出将解析的引用，写操作前按 provider/run policy 请求确认。

后续可为 AI 注入一个统一 resolver skill，例如：

```ts
interface ResolveAiContextReferencesRequest {
  workspaceId: string;
  references: AiContextReference[];
  detailLevel: "metadata" | "summary" | "full";
}
```

首批 resolver 可以映射到现有能力：

- session: `tendi sessions transcript` 或对应 Tauri session transcript 命令。
- skill: skill list/detail/file read 能力。
- issue/task/artifact/workspace-reference: Tutti CLI 或 workspace skill。

## 错误和安全约束

- 前端不得把敏感内容塞入 `metadata`。
- 后端校验失败的引用不能静默丢弃，必须回传 diagnostic。
- 发送前如果存在 `forbidden` 或 `unsupported-type`，默认阻止请求。
- `missing` 可以允许发送，但 AI prompt 中必须标注该引用缺失。
- `allowWrite` 默认为 `false`。即使引用包含 `write` 权限，也不代表 AI run 默认可以写。
- 引用草稿过期后需要重新 validate，不能直接复用旧权限快照。

## 最小实现顺序

1. 增加 `AiContextReference` 前端类型/构造 helper，先支持 session 和 skill。
2. 在 app-level state 增加 AI context store，支持 add/remove/dedupe/order/prompt draft。
3. 给 Sessions 和 Skills 页面接入“加入 AI 上下文”的批量操作。
4. 增加全局 AI 按钮和对话面板，展示引用篮子与 prompt draft。
5. 增加 `validate_ai_context_references` IPC，先返回存在性和权限 diagnostics。
6. 定义 provider request payload，并把引用 payload 传给后续 Codex/Cursor Cloud provider 接入任务。

