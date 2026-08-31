# Runtime Schema / Codegen 方案

## 结论

既然允许重构 daemon wire protocol，推荐采用 OpenRPC 作为请求—响应协议的唯一来源，并把 daemon 改成标准 JSON-RPC 2.0。Tendi 的完整协议源文件仍然只保留一份：

```text
runtime-schema/runtime.openrpc.json
        |
        +-- OpenRPC methods / params / result / errors
        +-- JSON Schema components.schemas
        +-- x-tendi SSE event / Tauri / capability metadata
        |
        +-- generated TypeScript types / validators / client
        +-- generated Rust types / validators / registry / dispatch glue
        +-- generated CLI client signatures
        +-- generated contract fixtures and test cases
```

OpenRPC 标准化 method、params、result、error、examples 和文档；JSON Schema 负责跨语言 DTO 约束；`x-tendi` 只承载 OpenRPC 没有覆盖的 SSE event、Tauri transport、owner 和 capability 元数据。命令和事件信息不再散落在 TypeScript、Rust、CLI 和 Tauri 注册表中。

这里不是把 OpenRPC 硬套到 SSE 上：OpenRPC 只描述 request/response command。SSE 事件继续使用 SSE transport，由同一文件中的 `x-tendi.events` 描述 payload、revision 和 replay 语义。Tauri 只作为 transport adapter；业务 command 仍由 OpenRPC method 描述。

这比继续维护一个完全自定义 manifest 更适合长期维护。代价是一次性的 wire breaking change：`{ command, args }`、`{ ok, result, error }` 和字符串错误码需要迁移为 JSON-RPC 2.0 envelope。

## 当前证据

当前仓库已有以下重复边界：

| 边界 | 当前实现 | 问题 |
| --- | --- | --- |
| 前端命令名 | `apps/desktop/src/lib/tauri.ts` 的 `TauriCommand` | 维护了一份命令名和一份 `DAEMON_COMMANDS` |
| daemon 命令注册 | `crates/tendi-daemon/src/lib.rs:715` 附近的 `dispatch` match | 与前端命令列表独立维护，且包含前端列表没有的命令 |
| Rust 请求 | `DaemonRequest { command: String, args: Value }` | 参数没有按命令静态建模 |
| Rust 响应 | `DaemonResponse { result: Option<Value> }` | 结果类型在运行时才知道 |
| Rust 事件 | `DaemonEvent { event: String, payload: Value }` | 事件名和 payload 没有联合类型约束 |
| TS DTO | `apps/desktop/src/lib/runtime-gateway.ts` | 大量手写类型、类型断言、逐个校验函数 |
| TS transport | `apps/desktop/src/lib/tauri.ts` | Tauri、web bridge、daemon command capability 各自判断 |
| CLI transport | `crates/tendi-cli/src/main.rs:352` 附近 | CLI 自己拼 `/v1/rpc` 请求和 command 字符串 |
| 事件 | daemon 常量和前端 `RuntimeEvent` 各维护一份 | `sessions://scan` 等事件协议重复 |

当前实际还存在 transport 控制命令和桌面专属命令，例如 `daemon_invoke`、事件订阅命令、`log_event`、更新、文件系统和应用启动命令。验收范围必须覆盖所有 `#[tauri::command]`、daemon dispatch arm、CLI daemon call 和前端 command 引用；不能只覆盖当前 `TauriCommand` enum。

## 目标

完成后必须满足：

1. 所有公共 command name 只在 `runtime-schema/runtime.openrpc.json` 出现一次；daemon 和 desktop command 作为 OpenRPC method，transport 控制项作为同文件的 Tendi transport metadata。
2. 所有跨边界 request、response、error、event payload 和 event envelope 都从 schema 生成。
3. Rust daemon handler 接收和返回生成的类型，不再使用 `Value` 作为业务 handler 的 request / response 类型。
4. TypeScript client 按 command name 自动得到 request 和 response 类型，并在 transport 边界进行运行时校验。
5. Rust dispatch、Tauri handler 注册表、CLI client method 和 GUI capability 列表都从同一 registry 生成。
6. GUI 和 CLI 继续保留各自 transport 实现，但不再各自维护 command/DTO 协议。
7. `runtime-gateway.ts` 只保留业务适配：normalize、provider/domain 转换、错误策略、`null` 策略和 Store/controller workflow。
8. 新增一个 command 时，新增 schema、生成代码和手写 handler；不再手改多个 command list、DTO 和 wrapper。

## 不在生成范围内

以下逻辑继续手写，且不放进 schema：

- provider 解析、provider capability 和 provider-specific path 规则；
- skill 更新、冲突、回滚、staging 和文件应用；
- daemon handler 内的业务流程和 `OperationCoordinator` 调度；
- DesktopStore reducer、页面状态和错误展示；
- CLI 输出格式；
- `normalizeSession`、transcript normalize 和业务层 domain conversion；
- 哪些调用失败返回 `null`，哪些调用必须抛错；
- Tauri、HTTP、SSE 的底层连接、重连、超时和认证细节。

生成 client 默认失败即抛出 typed error。`safeInvoke` 和其它 `null` 策略继续由业务调用方明确选择，不由 schema 自动推断。

## Schema 设计

### 文件形式

只保留 JSON 作为权威源。原因：

- JSON Schema 原生使用 JSON 和 `$ref`；
- Rust `jsonschema`、Ajv 和各类 codegen 工具直接消费 JSON；
- 避免 YAML parser、YAML 格式化和 YAML/JSON 双文件漂移；
- 人工阅读仍可通过拆分 `components.schemas`、description 和稳定排序解决。

主文件建议结构：

```json
{
  "openrpc": "<supported OpenRPC version>",
  "info": { "title": "Tendi Runtime", "version": "1.0.0" },
  "servers": [
    { "name": "daemon", "url": "/v1/rpc" },
    { "name": "desktop", "url": "tauri://invoke" }
  ],
  "methods": [
    {
      "name": "sessions_snapshot",
      "params": [],
      "result": {
        "name": "result",
        "schema": { "$ref": "#/components/schemas/SessionSnapshot" }
      },
      "x-tendi": {
        "owner": "daemon",
        "wire": "jsonrpc_http | tauri | web_bridge",
        "clients": ["desktop", "cli"],
        "execution": "read"
      }
    },
    {
      "name": "app_icon_set",
      "params": [
        {
          "name": "request",
          "required": true,
          "schema": { "$ref": "#/components/schemas/AppIconSetRequest" }
        }
      ],
      "result": {
        "name": "result",
        "schema": { "$ref": "#/components/schemas/Unit" }
      },
      "x-tendi": {
        "owner": "desktop",
        "wire": "tauri",
        "clients": ["desktop"],
        "execution": "write"
      }
    }
  ],
  "components": {
    "schemas": {
      "EmptyRequest": {
        "type": "object",
        "additionalProperties": false
      },
      "SessionSnapshot": {
        "type": "object",
        "properties": {
          "scopeKey": { "type": "string", "minLength": 1 },
          "domain": { "const": "sessions" },
          "revision": {
            "type": "integer",
            "minimum": 0,
            "maximum": 9007199254740991
          },
          "schemaVersion": { "const": 1 },
          "snapshotId": { "type": "string", "minLength": 1 },
          "payload": {
            "type": "array",
            "items": { "$ref": "#/components/schemas/SessionRecord" }
          }
        },
        "required": [
          "scopeKey", "domain", "revision",
          "schemaVersion", "snapshotId", "payload"
        ],
        "additionalProperties": false
      }
    }
  },
  "x-tendi": {
    "protocolVersion": 2,
    "events": [
      {
        "name": "sessions://scan",
        "payload": { "$ref": "#/components/schemas/SessionScanEvent" },
        "revisioned": true,
        "clients": ["desktop", "cli"]
      },
      {
        "name": "analytics://progress",
        "payload": { "$ref": "#/components/schemas/AnalyticsProgressEvent" },
        "revisioned": false,
        "clients": ["desktop"]
      }
    ],
    "transports": [
      { "name": "daemon_invoke", "kind": "tauri_bridge" },
      { "name": "daemon_next_event", "kind": "tauri_event_stream" },
      { "name": "log_event", "kind": "daemon_http_log" }
    ]
  }
}
```

OpenRPC 的 `methods[].params` 使用 named parameters。生成器把参数描述编译成 TypeScript request DTO 和 Rust request struct；无参数 method 生成 `EmptyRequest`，实际 JSON-RPC `params` 发送空对象。`x-tendi.transports` 只登记 transport plumbing，不把它伪装成业务 RPC method。

正式 schema 必须包含完整 inventory，不得以示例数量作为完成条件。

### JSON-RPC 2.0 wire 迁移

daemon 的公开 request/response 目标形状固定为：

```json
{
  "jsonrpc": "2.0",
  "id": "desktop-123",
  "method": "sessions_snapshot",
  "params": {}
}
```

成功响应为 `{ "jsonrpc": "2.0", "id": "desktop-123", "result": ... }`；失败响应为 `{ "jsonrpc": "2.0", "id": "desktop-123", "error": { "code": -32001, "message": "...", "data": { "kind": "..." } } }`。

JSON-RPC 的 `error.code` 是数字，因此现有字符串错误码不能原样保留。生成 schema 负责稳定的 numeric code 和错误 data 中的 symbolic `kind`；客户端只能按生成的 code/kind 分支，不能按 message 分支。批量请求、notification 和未声明 method 默认不启用。

HTTP `/v1/rpc`、Tauri invoke、web bridge 和 CLI 都只负责把各自 transport 转换到同一 JSON-RPC client。`/health` 继续负责握手，SSE `/v1/events` 继续负责事件流；两者不需要伪装成 JSON-RPC method。

### 必须明确的类型规则

- wire field 使用现有 camelCase 协议，例如 `scopeKey`、`baseRevision`、`updatedAt`。
- `optional` 和 `nullable` 分开建模。可缺省字段放进非 required；允许 `null` 的字段显式使用 `type: ["string", "null"]` 或等价结构。
- JavaScript 安全整数上限为 `9007199254740991`。超过该范围的 Rust `u64` 必须在 wire 上使用 decimal string，不能直接生成 TS `number`。
- Rust `Option<T>` 不能作为“可能缺失或可能为 null”的隐式兼容类型。schema 必须决定具体语义。
- DTO 对象默认 `additionalProperties: false`。允许动态 JSON 的位置必须命名为 `JsonValue` / `JsonObject`，并加 description 说明原因。
- `Record<string, unknown>`、`serde_json::Value` 只能存在于明确的动态字段或 raw transport ingress/egress，不能出现在业务 handler 的 request、response、event payload 签名中。
- 每个 method 必须有 result、owner、wire、clients 和 execution；无参数也生成 `EmptyRequest`，不使用隐式 `undefined`。
- 每个 event 必须有固定 event name、payload schema、wire 和 revisioned 语义。
- error code 是稳定的 numeric 协议字段，symbolic `kind` 放在 error data 中。message 可以变化，但不能被客户端作为分支条件。

### command 分类

协议源必须覆盖以下四类，不允许遗漏：

| 类别 | 例子 | 生成内容 |
| --- | --- | --- |
| daemon business | `sessions_snapshot`、`skills_list` | OpenRPC method、Rust handler/dispatch、TS client、CLI client |
| desktop-only | `open_url`、`install_update` | OpenRPC method + `x-tendi.owner=desktop`、Tauri 类型和 registration |
| transport control | `daemon_invoke`、`daemon_next_event`、`log_event` | `x-tendi.transports`、bridge 类型、事件订阅类型、日志请求类型 |
| legacy/internal | `scan`、旧 CLI 使用的 command | method 上明确 `deprecated` 或 `internal`，不能从 registry 中静默删除 |

`owner` 表示实现归属，不表示业务逻辑会由 generator 生成。handler 仍然是手写的。

## 生成物

建议把生成物提交到仓库，便于 IDE、rust-analyzer 和 TypeScript 编译器直接使用。生成文件顶部写入 `DO NOT EDIT`，CI 检查生成物是否与 schema 一致。

```text
runtime-schema/
  runtime.openrpc.json
  meta-schema.json
  examples/
    commands/
    events/

tools/runtime-codegen/
  README.md
  ...

apps/desktop/src/lib/generated/
  runtime-types.ts
  runtime-validators.ts
  runtime-client.ts
  runtime-events.ts

crates/tendi-core/src/generated/
  runtime_contract.rs

crates/tendi-daemon/src/generated/
  runtime_dispatch.rs
  runtime_events.rs

crates/tendi-cli/src/generated/
  runtime_client.rs
```

生成器由一个命令驱动，例如：

```text
npm run runtime:codegen
npm run runtime:codegen -- --check
```

实际命令可以放到 workspace `xtask`，但只能保留一个入口。不能让 Rust 和 TypeScript 分别有一套生成命令。

### TypeScript 生成物

生成：

- 所有 `components.schemas` 的 TypeScript 类型；
- `CommandName`、`RequestFor<C>`、`ResponseFor<C>`；
- `RuntimeEvent` 的 event-name discriminated union；
- Ajv 编译后的 request、response、event validators；
- `RuntimeClient` command methods；
- daemon/web/Tauri transport 所需的 envelope 类型；
- command capability 列表和 event capability 列表。

生成 client 的公开形状应接近：

```ts
client.sessionsSnapshot({}): Promise<SessionSnapshot>;
client.skillsList({}): Promise<SkillListResponse>;
client.openUrl({ url }): Promise<Unit>;
```

方法内部的 transport 选择和 response validation 可以共用手写的 transport adapter，但 command 到 request/response 的映射必须生成。

### Rust 生成物

生成：

- request / response / event DTO；
- `CommandName` 和 `EventName`；
- `CommandRequest` 和 `CommandResult` tagged enum；
- `RuntimeHandler` trait 签名；
- `dispatch` glue；
- `command_requires_serialized_write` 所需的 registry metadata；
- typed daemon error / response / event envelope；
- Rust client 的 typed method 签名。

handler 仍然手写，例如：

```rust
impl RuntimeHandler for Daemon {
    fn sessions_snapshot(
        &self,
        request: SessionsSnapshotRequest,
    ) -> HandlerResult<SessionsSnapshot> {
        // 业务逻辑仍然在这里
    }
}
```

生成的 dispatch 负责把 command name 解码为 `CommandRequest`，调用 trait 方法，再编码对应的 `CommandResult`。最终不再维护手写的字符串 match。

### Tauri 生成物

`tauri::generate_handler!` 需要 Rust 函数标识符，因此不能只生成一个字符串数组。生成器应输出 Tauri registration glue：

- schema 中的 desktop command 名称；
- 对应的 typed request/result；
- `generate_handler!` 所需的函数列表或宏展开文件；
- `daemon_invoke`、事件订阅和取消订阅的 typed boundary。

Tauri handler 内部仍然手写 OS 行为。生成器只负责注册和协议类型，不生成打开应用、更新安装或文件系统逻辑。

## Runtime validation

### TypeScript

使用 Ajv：

1. 生成或加载 `components.schemas` 和 `x-tendi` event schemas。
2. 启动时编译 validators。
3. 发送 request 前校验 args。
4. 收到 daemon、web bridge 或 Tauri response 后校验 result。
5. 收到 SSE/Tauri event 后先校验 envelope 和对应 payload，再交给 `RuntimeEvent` consumer。

校验失败必须产生包含 command/event、字段路径和 schema version 的 typed contract error。不能继续把无效 payload 传给 Store。

### Rust

Rust 生成的 serde 类型负责静态字段、枚举和基本反序列化。JSON Schema validator 负责 schema constraint，例如 `minLength`、`const`、`additionalProperties` 和安全整数范围。

边界顺序固定为：

```text
raw JSON
  -> generated schema validator
  -> generated request DTO
  -> hand-written handler
  -> generated response DTO
  -> generated schema validator
  -> raw JSON / Tauri result
```

不要把 `typify` 生成的 Rust 类型当成全部运行时约束。Typify 官方文档明确说明 JSON Schema 的复杂约束不一定能完整表达为 Rust 类型，尤其是 bounded numbers。因此必须保留 Rust 侧 JSON Schema validation。

## 迁移阶段

### 阶段 0：建立基线，不改行为

产物：`runtime-schema/inventory` 和 golden fixtures。

动作：

1. 收集所有 daemon dispatch arm、`TauriCommand`、`DAEMON_COMMANDS`、`DESKTOP_ONLY_COMMANDS`、`#[tauri::command]`、CLI daemon call、前端 `invokeCommand` 引用和 event constant。
2. 为每个 command 标记 owner、wire、client、execution 和当前 response fixture。
3. 为每个 event 收集成功、失败、revisioned、非 revisioned fixture。
4. 对当前 `Value` response 做真实运行时采样，不根据 TypeScript 类型猜测 schema。
5. 标出只有部分入口使用的命令，决定是公开、internal、deprecated 还是移除。

完成条件：inventory 能列出每个来源的 command/event，并且没有未分类项。此阶段不删除旧代码。

### 阶段 1：schema、meta-schema 和生成器

产物：`runtime.openrpc.json`、meta-schema、codegen command、Rust/TS 最小示例生成物。

动作：

1. 把 OpenRPC methods、JSON Schema components 和 Tendi metadata 写入 `runtime.openrpc.json`。
2. 用 OpenRPC meta-schema 和 JSON Schema validator 校验文档本身，检查 `$ref`、重复 method、重复 event 和缺失字段。
3. 接入 Rust 类型生成、TypeScript 类型生成、Ajv validator 和 Rust validator。
4. 生成一个无业务逻辑的 `ping` 或 `sessions_snapshot` vertical slice。
5. 把生成物加入 `codegen --check`。

完成条件：修改一个 `components.schemas` 字段后，Rust 和 TypeScript 类型、validator、fixture test 都发生对应变化；手写旧 DTO 不参与新 slice。

### 阶段 2：迁移 daemon boundary

动作：

1. 用生成的 `JsonRpcRequest`、`JsonRpcResponse`、`JsonRpcError` 和 per-method request/result DTO 替换 `DaemonRequest.args: Value` 作为 handler 输入。
2. 让 raw JSON adapter 解析标准 `method`、`params`、`id`，再进入 generated typed dispatch；删除手写 command 字符串 match。
3. 把 `DaemonEvent.payload: Value` 替换为生成的 event union。
4. 将 `command_requires_serialized_write` 的 command 列表迁移到 schema metadata；条件执行规则使用生成的 metadata 加手写 predicate。
5. 保持 `handle_json_rpc(Value) -> Value` 作为最外层 JSON-RPC adapter，但只允许 `Value` 停留在 ingress/egress；不允许进入 handler trait。

完成条件：daemon 的每个 dispatch command 都经过 schema request validation；每个成功 response 和 event 都经过 schema response validation；现有 handler 行为和 revision/event 测试不变。

### 阶段 3：迁移 GUI 和 web bridge

动作：

1. 用生成的 `RuntimeClient` 替代 `TauriCommand`、`DAEMON_COMMANDS` 和 `DESKTOP_ONLY_COMMANDS`；Tauri 仅实现 generated transport interface。
2. 把 Tauri、web bridge 和 SSE 适配器改为实现统一的 generated transport interface。
3. 删除 `runtime-gateway.ts` 中 command DTO、event DTO、重复的结构校验。
4. 保留 `normalizeSession`、transcript normalize、错误展示和 `null` 策略。
5. 将 Store reducer 的 event 输入改为 generated `RuntimeEvent`，继续保留 revision decision 业务规则。

完成条件：前端不再手写 command 字符串集合；所有 command call 的 args 和返回值由 `RuntimeClient` 推断并运行时校验；GUI/web 两种运行模式的现有行为一致。

### 阶段 4：迁移 CLI

动作：

1. 用生成的 Rust client method 替代 CLI 中的 command 字符串和 `serde_json::json!({ "jsonrpc": "2.0", "method": ..., "params": ... })`。
2. 将现有 `daemon_http_json` 收敛为 transport adapter，只负责 TCP/HTTP、health、token、超时和连接失败。
3. CLI workflow 继续手写输出格式和 daemon 不可用时的本地 Store fallback。
4. CLI 与 daemon 共享生成的 request/response/event contract。

完成条件：CLI 不再手写 daemon command name、request DTO 或 response DTO；CLI 输出快照与迁移前一致。

### 阶段 5：删除重复源并锁定 CI

删除：

- 手写 `TauriCommand` enum；
- 手写 `DAEMON_COMMANDS` 和 `DESKTOP_ONLY_COMMANDS`；
- 手写 `DaemonRequest` / `DaemonResponse` 的非标准 `{ command, args }` / `{ ok, result, error }` 协议；
- 手写 `DaemonEvent` payload `Value` 协议；
- `runtime-gateway.ts` 内只用于 transport contract 的 DTO；
- CLI 的 command 字符串和 command-specific JSON 拼装；
- daemon 的 command 字符串 dispatch match；
- 未被 schema 标记的 `Record<string, unknown>` command args 和 `Value` response。

保留：

- provider/domain 类型和转换；
- handler 业务逻辑；
- transport 连接实现；
- Store、controller、页面和 CLI workflow；
- 明确的 `safeInvoke` / `null` / error display 策略。

## 可验收标准

### A. 唯一来源

- `runtime-schema/runtime.openrpc.json` 是唯一 command/event/DTO 权威源。
- 新增 command 只需新增一个 OpenRPC method、对应 request/result `components.schemas` 和手写 handler。
- CI 扫描源码时，手写层不得新增 command name 字符串集合。
- 所有旧来源都能由 inventory script 找到并与 schema 一一对应；缺失、重复、孤儿都失败。
- `TauriCommand`、`DAEMON_COMMANDS`、Rust dispatch match、CLI command 拼装全部被删除或变成生成文件。

### B. 生成完整性

- 每个 command 都生成 Rust request、Rust response、TypeScript request、TypeScript response、client method 和 capability metadata。
- 每个 event 都生成 Rust payload、TypeScript payload、event union 和 validator。
- 每个 command/event 都有至少一个 valid fixture 和至少一个 invalid fixture。
- `runtime:codegen --check` 在未重新生成时失败。
- 生成结果经过 `rustfmt`、TypeScript formatter 和稳定排序；重复运行生成器不会产生 diff。

### C. 边界类型安全

- daemon handler 不再接收 `&Value` 作为 command args。
- daemon handler 不再返回 `Result<Value, DaemonError>` 作为 command response。
- `DaemonEvent.payload` 不再是 `Value`。
- GUI command call 不再使用 `invokeCommand<Record<string, unknown>>` 表示有明确 schema 的 DTO。
- 所有 request、response、event 在 Rust 和 TypeScript 两侧都进行运行时 schema validation。
- 未知 command、缺少 required field、错误 enum、未知字段、错误 nullable/optional 语义和超出安全整数范围都会返回 typed contract error。
- daemon RPC 成功/失败 envelope 符合 JSON-RPC 2.0；不存在旧的 `ok` 分支协议，错误码使用稳定 numeric code。

### D. Transport 一致性

- 同一个 daemon command 通过 Tauri embedded daemon、web bridge、standalone HTTP daemon 和 CLI 得到相同 JSON contract。
- `/v1/rpc`、web bridge、Tauri 和 CLI 对同一 method 使用相同的 `jsonrpc`、`id`、`method`、`params`、`result`/`error` 语义。
- SSE 的 `id`、`event`、`data` framing 仍由 transport adapter 处理，但 `data` 解码后的 event 必须通过 generated union validator。
- Tauri-only command 在 web runtime 中明确返回 unsupported error，不会被当成 daemon command 发送。
- CLI 无 daemon 时的本地 Store fallback 不改变；fallback 的选择仍由 CLI workflow 决定，不由 schema 生成。
- daemon health 或握手返回 protocol/schema version；版本不匹配返回明确错误，不执行未知 contract。

### E. 行为不回归

- 现有 sessions snapshot、revision gap、event replay、skill update、config watch、backup 和 file mutation 测试继续通过。
- valid golden fixtures 的 JSON 结构保持不变，除非 schema 变更记录明确的 breaking change。
- GUI Store reducer 收到的 event 顺序、`baseRevision -> revision` 判断和 resync 行为保持不变。
- CLI 的表格、JSON、错误退出码和 fallback 行为保持不变。
- provider 解析测试、skill rollback 测试和 controller 测试不因 codegen 迁移而改成 schema 特例。

### F. 可维护性验收

验收者执行以下流程：

1. 在 schema 增加一个带参数、带 response DTO 和带 event payload 的临时 command。
2. 运行一次 codegen。
3. 检查 Rust handler trait、Rust dispatch、TypeScript client、TypeScript validator、CLI client 和 fixture test 均出现对应生成结果。
4. 只新增一个手写 daemon handler，不修改 command list、Tauri capability list、CLI transport 和前端 DTO。
5. 删除临时 command，再次运行 codegen，所有生成物和 fixture 都被删除。

以上流程通过，才算“命令名、DTO、协议和 wrapper 自动生成”真正完成。

## CI 检查入口

建议增加：

```text
npm run runtime:validate-schema
npm run runtime:codegen -- --check
npm run runtime:test-contract
cargo test -p tendi-core runtime_contract
cargo test -p tendi-daemon runtime_contract
cargo test --workspace
npm run typecheck
```

其中 `runtime:validate-schema` 校验 OpenRPC 文档、`components.schemas` 和 `x-tendi` 扩展；`runtime:test-contract` 至少覆盖：

- schema meta-validation；
- 所有 `$ref` 可解析；
- command/event inventory 对账；
- valid/invalid request；
- valid/invalid response；
- valid/invalid event；
- Rust serialize → TypeScript validate；
- TypeScript serialize → Rust deserialize；
- unknown command 和 protocol version mismatch。

## 开源方案调研

### 推荐组合：JSON Schema + 薄生成器

| 组件 | 用途 | 结论 |
| --- | --- | --- |
| JSON Schema Draft 2020-12 | DTO 约束、`$ref`、nullable、additional properties | 作为唯一 DTO 来源 |
| `typify` | JSON Schema → Rust 类型 | 可用，但不能替代运行时约束 |
| `json-schema-to-typescript` | JSON Schema → TypeScript declaration | 适合生成前端 DTO 类型 |
| Ajv | TypeScript runtime validation | 适合 request/response/event 边界校验 |
| Rust `jsonschema` | Rust runtime validation | 适合嵌入生成 schema 并校验 wire JSON |
| Tendi generator | command registry、event union、client、dispatch、fixture | 必须自定义，但范围可以保持很小 |

Typify 官方定位是 JSON Schema 到 Rust 类型生成，并支持 CLI、macro 和 build generator；其文档同时明确指出 JSON Schema 的复杂约束不一定能完整映射成 Rust 类型，因此本方案把它限定为类型生成器，不把它当作 validator。[Typify](https://github.com/oxidecomputer/typify)

`json-schema-to-typescript` 直接把 JSON Schema 编译成 TypeScript typings。[json-schema-to-typescript](https://github.com/bcherny/json-schema-to-typescript)

Ajv 提供 JSON Schema 的 TypeScript type guard、compiled validator 和 type-safe error 能力。[Ajv TypeScript 文档](https://ajv.js.org/guide/typescript.html)

Rust `jsonschema` 支持 Draft 2020-12，并提供编译期 validator macro 和运行时 validator。[Rust jsonschema 文档](https://docs.rs/jsonschema/latest/jsonschema/)

### 推荐组合：OpenRPC + JSON Schema + Tendi generator

OpenRPC 是描述 JSON-RPC 2.0 API 的标准，适合表达 method、params、result、error、examples 和版本化文档。[OpenRPC specification](https://spec.open-rpc.org/)

OpenRPC 官方 generator 有 client、server、documentation 组件，并支持自定义 component。[OpenRPC generator](https://github.com/open-rpc/generator)。它可以复用在标准 method 文档、示例和部分 TypeScript client 生成上，但官方模板目录当前没有现成的 Tendi Rust daemon dispatch 模板。[OpenRPC templates](https://github.com/open-rpc/generator/tree/master/templates)

因此最终方案不是“只运行 OpenRPC generator”，而是：

- OpenRPC `methods` 作为 command name、params、result、errors 和 examples 的唯一来源；
- OpenRPC `components.schemas` 作为 request/result/event payload 的 JSON Schema 来源；
- Tendi generator 只补齐 Rust typed dispatch、Tauri registration、SSE event union、transport metadata、双方 validator 和 contract fixtures；
- `x-tendi.events` 描述 OpenRPC 没有覆盖的 SSE event name、payload、revision、replay 和 client capability；
- `x-tendi` method metadata 标记 daemon、desktop-only、CLI/GUI client 和执行类别。

这是本次允许重构后的首选。若不迁移 JSON-RPC 2.0，才退回“纯 JSON Schema 自定义 manifest”；那条路径迁移风险较低，但会继续自行维护 method registry 和非标准 request/response envelope。

### Protobuf + Buf

Buf 能从 `.proto` 通过插件生成 TypeScript、Rust 等多语言代码，并提供 lint、module 管理和 breaking change 检查。[Buf code generation](https://buf.build/docs/generate/)

Prost 是 Rust 的 Protocol Buffers 代码生成和运行库。[Prost](https://github.com/tokio-rs/prost)

不推荐当前采用：

- 现有 daemon、CLI、web bridge 和 Tauri 都以 JSON 为边界；
- protobuf 会带来二进制编码、`proto3` optional/oneof 语义、插件和 generated service 的迁移；
- SSE 和 Tauri event 仍要另建协议；
- 不能直接消除 Tendi 的 handler、provider、Store 和 UI workflow glue。

如果未来需要跨进程高吞吐、非 JSON 客户端或标准 streaming RPC，再单独评估 Buf/Protobuf。

### OpenAPI + Utoipa + openapi-typescript

Utoipa 是 Rust code-first OpenAPI 生成工具，`openapi-typescript` 是 OpenAPI 3 到 TypeScript 类型生成工具。[Utoipa](https://github.com/juhaku/utoipa)、[openapi-typescript](https://github.com/openapi-ts/openapi-typescript)

不推荐当前作为唯一来源：OpenAPI 主要描述 HTTP resource/path，Tendi 的核心是 command RPC、Tauri invoke 和 SSE event。Utoipa 也属于 Rust-first code-first，无法满足“协议文件作为唯一来源”。它可以在未来 daemon 暴露 REST API 时使用。

### Quicktype

Quicktype 支持从 JSON Schema 生成 Rust、TypeScript 等多语言类型和 JSON converter，并建议把审核后的 schema 提交到仓库，再从 schema 生成代码。[Quicktype](https://github.com/glideapps/quicktype)

它适合做 POC 或替代基础 DTO generator，但 command registry、Tauri handler registration、typed event union、Tendi error policy 和 contract fixture 仍要自定义。因此本方案不把 Quicktype 当成完整 RPC codegen。

### Rust-first：Specta、ts-rs、Typeshare

Specta、ts-rs 和 Typeshare 都适合从 Rust 类型生成 TypeScript。Specta 官方说明它通过 Rust 类型 introspection 导出 TypeScript；Typeshare 通过分析 Rust 源文件生成绑定。[Specta](https://docs.rs/specta/latest/specta/)、[ts-rs](https://docs.rs/ts-rs/latest/ts_rs/)、[Typeshare](https://docs.rs/typeshare/latest/typeshare/)

不推荐当前采用：它们会把 Rust 类型重新变成 source of truth，无法解决用户现在指出的“Rust `Value` 与前端手写 DTO 两份契约”问题，也不会自动提供 Tendi command/event registry 和双方 runtime validator。

## 主要风险和处理

| 风险 | 处理 |
| --- | --- |
| 现有 `Value` 结构比手写 TS 类型更宽 | 阶段 0 使用真实 response fixture；不从 TS 类型反推 schema |
| `Option`、null、缺省字段语义混乱 | schema 明确 `required` 和 nullable；invalid fixture 锁定行为 |
| Rust 类型生成无法表达 bounded number | Rust JSON Schema validator 补足运行时约束 |
| Tauri macro 不能直接读取 JSON registry | 生成 Rust registration glue，不要求 handler 业务自动生成 |
| CLI 与 GUI transport 不同 | 共享 generated contract/client signature，transport adapter 继续分别手写 |
| hidden/legacy command 被迁移时遗漏 | inventory 对账；必须标记 `internal` 或 `deprecated`，不能静默删除 |
| generator 自身成为新的胶水黑盒 | 只生成 schema 映射、registry、validator、wrapper 和 fixtures；模板小且有 golden tests |
| schema 变更破坏已安装 daemon/CLI | 使用 `protocolVersion` / `schemaVersion` health 检查；不添加未证实需要的 legacy fallback |

## Definition of Done

当且仅当以下条件同时满足，项目可以声称 runtime contract 已完成 schema codegen：

1. inventory 中的所有 command/event 都在 `runtime.openrpc.json` 中有唯一 entry。
2. Rust daemon、Tauri、GUI、CLI 都使用生成的 command/event/DTO registry。
3. Rust handler、TS gateway 和 CLI workflow 中不再通过 `Value` / `unknown` / `Record<string, unknown>` 表示已知协议。
4. 请求、响应、事件在 Rust 和 TypeScript 两侧都有运行时校验。
5. codegen 可重复执行，`--check` 能阻止 stale generated files。
6. 跨语言 fixtures、invalid contract tests、现有 daemon/GUI/CLI 行为测试全部通过。
7. 新增临时 command 的验收流程不需要手改第二份 command 名称或 DTO。
