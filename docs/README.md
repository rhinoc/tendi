# Documentation

Project documentation is split between user-facing setup and focused implementation notes.

## User and contributor documentation

- [Project README](../README.md): features, requirements, installation, CLI usage, and local data.
- [Contributing guide](../CONTRIBUTING.md): development setup, checks, coding rules, and pull
  request expectations.
- [Security policy](../SECURITY.md): private vulnerability reporting and security scope.
- [License](../LICENSE): MIT license for the workspace source.

## Design and audit notes

- [AI context reference model](ai-context-reference-model.md): proposed reference-only model
  for carrying sessions, skills, and other local entities across an AI conversation.
- [`skills` CLI compatibility audit](skills-cli-compat-audit.md): database-authoritative lock
  migration, source/update compatibility, and remaining feature gaps.
- [MCP and Hooks tab audit](mcp-hooks-tab-audit.md): current data sources, UI capabilities,
  limitations, and follow-up design options.
- [Performance gates](performance-gates.md): CI fast and local performance thresholds, baselines,
  and the pre-push hook.
- [Logging](logging.md): persistent desktop and Rust logs, rotation, environment overrides, and
  the frontend logging rule.
- [Tauri updater](UPDATER.md): signing keys, release artifacts, `latest.json`, and update behavior.
- The local DMG workflow is documented in the [contributing guide](../CONTRIBUTING.md#development-setup)
  and implemented by `scripts/build-release.sh` and `scripts/build-dmg.sh`.

The design and audit notes describe current or proposed implementation details. When they differ
from the code, treat the code and the user-facing README as authoritative and update the note as
part of the related change.
