# `skills` CLI compatibility audit

Baseline: [`vercel-labs/skills`](https://github.com/vercel-labs/skills) `1.5.22`, commit
`c6f69c631292444cc541ac6d91e2226b0ff247da` (2026-08-10).

## Source database and lock migration

Tendi owns source provenance in SQLite's `skill_sources` table. It can migrate both lock formats
written by `skills`:

- Global v3: `$XDG_STATE_HOME/skills/.skill-lock.json`, falling back to
  `~/.agents/.skill-lock.json`.
- Project v1: `skills-lock.json` in the active project.

The database is authoritative per installed path. Scanning and project restore first query
`skill_sources`. If a row exists, Tendi does not use that skill's lock entry. If a row is absent,
Tendi may import the lock entry once, including `source`, `sourceUrl`, `sourceType`, `ref`,
`skillPath`, `skillFolderHash`, and `computedHash`. Tendi never rewrites either lock format.

Source rows survive ordinary scan disappearance, so a temporary missing path cannot make a stale
lock authoritative again. Explicit uninstall removes the exact installed-path row; a later
restore can then migrate that entry again. A later explicit install to the same path upserts the
database row.

Project restore is available through `tendi skills restore`. It restores missing v1 lock entries
to `.agents/skills`, persists their source rows, and leaves the lock byte-for-byte unchanged.
Ambiguous generic Git/GitLab shorthands without `sourceUrl` are rejected. `node_modules` entries
are reported but not restored.

## Implemented high-priority compatibility

- GitHub association, refs, repository-relative `skillPath`, and stored hashes.
- Database-backed update of copied/materialized GitHub, GitLab, generic Git, and Hugging Face
  skills. Updates replace the selected skill directory and advance only the SQLite source record.
- Global and project install scopes in CLI and desktop add flows.
- All 76 target identifiers from `skills` 1.5.22, plus legacy `shared` and `claude` aliases.
- GitHub shorthand/HTTPS/SSH, Git refs and tree subpaths.
- GitLab.com and self-hosted GitLab HTTPS/SSH, subgroups, refs, and tree subpaths.
- Hugging Face models, datasets, spaces, refs, and subpaths.
- Agent Skills well-known v0.1/v0.2, digest validation, direct `SKILL.md`, and bounded ZIP/TAR.GZ
  extraction with path traversal protection.

## Remaining gaps

| Capability | Tendi | `skills` 1.5.22 | Priority |
| --- | --- | --- | --- |
| Write `skills` lock files | Intentionally no; SQLite remains authoritative | Writes on add/update/remove/sync | Not planned under the DB-authority model |
| `node_modules` discovery | Restore reports and skips these entries | `experimental_sync` | Medium |
| Private GitHub fallback | Uses normal Git credentials | Also tries GitHub CLI and SSH fallback | Medium |
| Use without install | No | `use`, prompt output, optional agent launch | Medium |
| Deep/plugin discovery | Known roots and Codex plugin cache | `--full-depth` and plugin manifests | Medium |
| Eve subagent placement | Eve project root only | Tracks Eve subagents in project lock | Medium |
| Skill initialization | Desktop editor and wrapper creation | `init [name]` template | Low |
| Legacy Mintlify ingestion | Well-known/direct `SKILL.md` only | Legacy provider behavior exists in older releases | Low |
| Marketplace search/audit | Deliberately omitted | `find` and skills.sh public security metadata | Deferred by product decision |

Tendi additionally provides visibility policy, wrappers, dependency graphs, file editing, and
session-to-skill usage data, which the reference CLI does not provide.
