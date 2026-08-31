<div align="center">
  <br />
  <img src="./apps/desktop/src-tauri/icons/tendi-icon.svg" alt="Tendi 应用图标" width="112" height="112" />
  <h1>tendi</h1>
  <p>
    面向 <strong>Codex</strong>、<strong>Cursor</strong>、<strong>Claude Code</strong> 的本地优先 macOS 应用。<br />
    浏览会话、管理 skills，并把 rules、hooks、MCP 与配置放在同一处；需要时也可用配套 CLI。
  </p>
  <p>
    <a href="./README.md">English</a>
    &nbsp;·&nbsp;
    <a href="https://github.com/rhinoc/tendi/releases">Releases</a>
    &nbsp;·&nbsp;
    <a href="./LICENSE">License</a>
    &nbsp;·&nbsp;
    <a href="./CONTRIBUTING.md">Contributing</a>
  </p>
  <br />
</div>

## 截图

<table>
  <tr>
    <td align="center">
      <img src="./docs/screenshots/showcase-overview.jpg" width="250" alt="Tendi 总览：用量分析与最近会话" />
      <br />
      <sub>总览与用量趋势</sub>
    </td>
    <td align="center">
      <img src="./docs/screenshots/showcase-skills.jpg" width="250" alt="Tendi skills 库存与可见性控制" />
      <br />
      <sub>Skills</sub>
    </td>
    <td align="center">
      <img src="./docs/screenshots/showcase-sessions.jpg" width="250" alt="Tendi 会话列表与 transcript 详情" />
      <br />
      <sub>会话</sub>
    </td>
  </tr>
</table>

## 功能

- 📊 **用量** — Overview 可按会话、turn、模型、tool 和 skill 等维度查看 token 用量与趋势。单个会话可查看缓存率与 token 用量。
- 🧩 **Skills** — 安装、编辑、更新，并设置可见性（`auto` / `manual` / `off`）。远端 skill 的本地修改，在上游更新时通过三方合并保留。
- 💬 **会话** — 以接近 IM 的线程浏览 transcripts，包括 Agent 界面里常被省略的注入提示词与工具调用细节，并可从当前会话直接 resume。
- 🎛️ **配置** — 快速切换 Agent 的 config profiles，例如轮换 API key，而不必在各 provider 目录中查找文件。
- 📜 **Rules、hooks 与 MCP** — 在同一处浏览各 provider 的配置、打开源文件，并执行已支持的修改。
- 🔄 **同步** — 将 skills、MCP、rules、hooks 快照到 Git 仓库，并按明确计划恢复。
- 🖥️ **桌面与 CLI** — 应用与 `tendi` 共用同一套本地扫描器与快照数据库。

## 为什么做 Tendi

Tendi 面向已经同时使用多个 coding agent 的人。

在这种用法下，会话往往分散在不同应用里，skills 会越积越多并停在 `auto`，远端安装的 skill 也不好改——下一次更新可能覆盖本地修改。Tendi 把 Codex、Cursor、Claude Code 收拢到同一个本地界面，并补上几处实际缺口：

- 用不到的 skill 若保持 `auto`，描述仍可能每轮进入上下文。可在同一列表中改为 `manual` 或 `off`。
- 跨 Agent 搜索、查看并 resume 会话，避免换工具后重新交代上下文。
- 本地编辑已安装的远端 skill；上游更新时走三方合并，而不是覆盖或另存一份。

如果只用一个 Agent，也很少管理 skills 或回看旧会话，直接使用各 Agent 应用通常就足够。

## 要求

- 桌面应用目前需要 **Apple Silicon 的 macOS**。
- 运行时仅在 skill 源、marketplace 搜索、更新检查，或你配置的 sync 远程需要时访问网络。

## 安装

从 **[GitHub Releases](https://github.com/rhinoc/tendi/releases)** 下载最新的 **`tendi-<version>-aarch64.dmg`**。

1. 打开 DMG。
2. 将 **`tendi.app`** 拖到 **Applications**。
3. 推出磁盘映像，从「应用程序」或 Spotlight 启动 **Tendi**。

应用同时附带 `tendi` 命令行。可在首次运行提示中安装，之后也可在 **Settings → Developer → Coding helpers** 安装。

已安装的应用可在 **Settings → Updates** 检查更新。

### 首次启动与 Gatekeeper

浏览器下载会带上 Gatekeeper 隔离属性；当前发布版和本地构建均未使用 Apple Developer ID 签名，也未公证。若 macOS 无法打开应用，请先确认 DMG 来源可信，将 `tendi.app` 放入 Applications，然后执行：

```bash
xattr -dr com.apple.quarantine /Applications/tendi.app
```

## 使用

### 桌面应用

包含 **Overview**、**Skills**、**Sessions**、**Rules**、**MCP**、**Hooks**、**Prompts**、**Config**、**Settings**。

常见路径：

- **Skills → Add** — 粘贴 Git URL 或路径，或搜索 marketplace，预览后安装。
- **Sessions** — 搜索 transcripts，打开详情，resume 到 Codex、Cursor 或 Claude Code。
- **Settings → Developer → Sync** — 指定 Git 仓库，选择同步内容，同步或从历史恢复。

### CLI

安装 CLI 之后：

```bash
tendi scan
tendi skills list
tendi sessions search "your query"
tendi rules list
tendi hooks list
tendi mcp list
```

会修改文件的命令会先展示计划。`--dry-run` 只预览不写入，`--yes` 跳过确认。

安装 CLI 时也可以一并装上 Tendi 自带的 skill（目录在 `~/.agents/skills/tendi`），方便 coding agents 搜索本地会话和管理 skills。不想装可以跳过，之后也能在 **Settings → Developer → Coding helpers** 再装。

完整命令见 `tendi --help`。

### 本地数据

| 位置 | 内容 |
| --- | --- |
| `~/Library/Application Support/tendi/tendi.sqlite3` | 本地快照数据库 |
| `~/.agents/skills`、`~/.codex/skills`（设置 `CODEX_HOME` 时为 `$CODEX_HOME/skills`）、`~/.cursor/skills`、`~/.claude/skills` | Tendi 扫描的全局 skill 根目录 |
| 项目内 `.agents` / `.codex` / `.cursor` / `.claude` | 选定项目后的 skills、rules、hooks 与 MCP |

扫描完全基于本地文件。远程访问只发生在 skill 源、marketplace 或更新检查，以及你配置的 sync 远程。

## 参与贡献

开发环境、约定、测试、资源与发布边界见 **[CONTRIBUTING.md](./CONTRIBUTING.md)**。专题说明见 **[docs/](./docs/README.md)**。

## 许可证

Rust workspace 与源码仓库使用 [MIT License](LICENSE)。第三方依赖与捆绑资源保留各自许可证与归属要求。

应用图标与 DMG 背景为本项目资源。新增字体、媒体或其他二进制资源，进入公共仓库前需附带来源、许可证、归属与再分发条款。
