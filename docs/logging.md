# Logging

桌面应用和 Rust daemon 使用同一个持久化日志文件：

`~/Library/Application Support/tendi/logs/tendi.log`

前端调用 `apps/desktop/src/lib/logger.ts`。Tauri 模式通过 `log_event` 命令写入；web 模式通过 `/__tendi/log` 转发给 Rust daemon。Rust 启动时初始化同一个日志 writer，因此前端和 Rust 日志可以按 `pid`、`component` 和 `time` 对齐。

日志行是结构化文本：

```text
time=2026-08-18T10:00:00+08:00 level=warn component="tendi-desktop" pid=123 msg="daemon request failed" command="sessions_snapshot"
```

活动文件名固定为 `tendi.log`。跨日期或超过大小上限时轮转为 `tendi.YYYY-MM-DD.log`，并自动清理历史文件。

可用环境变量：

- `TENDI_LOG_PATH`：覆盖完整日志文件路径。
- `TENDI_LOG_DIR`：覆盖日志目录，文件名仍为 `tendi.log`。
- `TENDI_LOG_LEVEL`：`debug`、`info`、`warn`、`error`，默认 `info`。
- `TENDI_LOG_MAX_SIZE_MB`：单文件大小，默认 `50`。
- `TENDI_LOG_MAX_BACKUPS`：保留轮转文件数，默认 `10`。
- `TENDI_LOG_MAX_AGE_DAYS`：历史文件最长保留天数，默认 `14`。
- `TENDI_LOG_MAX_TOTAL_MB`：日志目录总大小，默认 `300`。

桌面生产代码禁止直接调用浏览器控制台 API。统一使用 `logger.debug/info/warn/error`，由 `check:no-browser-console` 检查。
