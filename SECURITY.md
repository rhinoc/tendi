# Security policy

## Reporting a vulnerability

Please do not disclose vulnerabilities, exposed credentials, local-data leaks, or update
channel issues in a public issue or pull request.

If GitHub private vulnerability reporting is enabled for this repository, use the **Security**
tab to create a private report. Otherwise, contact the repository maintainer privately and
include:

- affected commit, version, or build;
- operating system and agent/provider involved;
- reproducible steps or a minimal proof of concept;
- impact and any known mitigation.

Remove secrets and personal paths from the report where possible. If a credential may have been
exposed, revoke or rotate it immediately and report the affected service separately.

## Scope

Security-sensitive areas include:

- parsing and displaying local agent configuration, hooks, MCP settings, and transcripts;
- file-changing commands for skills, configuration, hooks, and prompts;
- remote Git, GitHub, registry, and HTTP skill sources;
- Tauri commands crossing between the desktop UI and local filesystem;
- release artifacts and update metadata, if publishing is introduced.

## Supported versions

Until published releases exist, security fixes target the latest `main` branch. There are no
supported binary releases at this time.
