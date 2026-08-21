---
name: tendi-log-debugging
description: Evidence-driven diagnosis of Tendi frontend, Tauri Rust, embedded or standalone daemon, web bridge, and persistent file logging. Use when logs are missing, malformed, not rotated, written to the wrong path, when frontend-to-Rust logging fails, when daemon requests fail, or when a runtime issue must be reproduced and verified in this repository.
---

# Tendi Log Debugging

按边界定位第一个失败点。先取证，再修改。保留工作区已有改动。

## 日志拓扑

- 默认文件：`~/Library/Application Support/tendi/logs/tendi.log`。
- `TENDI_LOG_PATH` 覆盖完整文件路径；`TENDI_LOG_DIR` 覆盖目录。
- `TENDI_LOG_LEVEL` 支持 `debug`、`info`、`warn`、`error`，默认 `info`。
- 前端入口：`apps/desktop/src/lib/logger.ts`。
- Tauri 通道：`apps/desktop/src-tauri/src/lib.rs` 的 `log_event`。
- web 通道：`apps/desktop/scripts/web-daemon.mjs` 的 `/__tendi/log`，再转到 Rust daemon 的 `/v1/log`。
- Rust writer：`crates/tendi-core/src/logging.rs`。
- daemon 启动：`crates/tendi-daemon/src/main.rs`；HTTP 边界：`crates/tendi-daemon/src/lib.rs`。

所有运行时日志使用 `logger.debug/info/warn/error` 或 Rust logger。禁止恢复浏览器控制台 API和 Rust 的运行时 `println!`/`eprintln!`。

## 排查流程

1. 记录当前状态：

   ```sh
   git status --short
   rg -n "TENDI_LOG_|log_event|/__tendi/log|/v1/log|logger\." apps/desktop crates docs
   ```

2. 确认实际文件：

   ```sh
   env | rg '^TENDI_LOG_(PATH|DIR|LEVEL|MAX_)'
   find ~/Library/Application\ Support/tendi/logs -maxdepth 1 -type f -print 2>/dev/null
   ```

   读取活动文件和最近轮转文件。不要先删除日志或重置工作区。

3. 按顺序检查链路：

   - 前端没有记录：检查 `logger` 的队列、`isTauriRuntime()`、Tauri `invoke` 或 web `fetch`。
   - Tauri 模式没有记录：检查 `log_event` 是否注册、Rust logger 是否在 `run()` 前初始化、路径是否可创建。
   - web 模式没有记录：检查 `/__tendi/log`、daemon token、`/v1/log` 授权和请求体。
   - Rust 有调用但文件为空：检查 `TENDI_LOG_LEVEL`、writer 锁、文件权限和轮转逻辑。
   - 只有轮转文件：检查日期变化、`TENDI_LOG_MAX_SIZE_MB`、`TENDI_LOG_MAX_BACKUPS`、`TENDI_LOG_MAX_AGE_DAYS` 和 `TENDI_LOG_MAX_TOTAL_MB`。

4. 用隔离路径复现 daemon 写入：

   ```sh
   test_dir="$(mktemp -d)"
   test_log="$test_dir/tendi.log"
   TENDI_LOG_PATH="$test_log" TENDI_DAEMON_TOKEN=test \
     cargo run -p tendi-daemon -- --port 5197 --workspace "$PWD" --token test
   ```

   另一个终端请求：

   ```sh
   curl -fsS -H 'Authorization: Bearer test' \
     -H 'Content-Type: application/json' \
     -d '{"level":"warn","message":"logging smoke test","fields":{"request_id":"smoke-1"}}' \
     http://127.0.0.1:5197/v1/log
   rg -n 'logging smoke test|request_id="smoke-1"' "$test_log"
   ```

5. 修改后验证：

   ```sh
   cargo test -p tendi-core logging
   cargo check --workspace
   npm --prefix apps/desktop run typecheck
   npm --prefix apps/desktop run check:no-browser-console
   npm --prefix apps/desktop run build
   git diff --check
   ```

## 修改规则

- 复用现有 logger 和日志路径，不在页面、daemon 或脚本中创建第二套 logger。
- 错误使用结构化字段记录；不要记录 token、完整 transcript 或大段请求体。
- 不用静默 fallback 掩盖文件打开、路径解析或 bridge 授权错误；保留错误证据。
- 热路径只记录状态变化和失败；成功的逐事件诊断使用 `debug`。
- 修改前端时同步检查 Tauri 和 web 两条通道。
- 修改轮转时同时覆盖活动文件、日期轮转、大小轮转和清理预算。

## 交付报告

说明：第一个失败边界、日志文件路径、证据命令和结果、修改文件、验证命令，以及没有完成的检查。若工作区已有未提交改动，单独说明，不能把它们归入本次修复。
