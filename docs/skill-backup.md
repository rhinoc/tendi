# Backup

Tendi can snapshot global managed skills, MCP, rules, and hooks into a Git
repository. All four categories are included by default. Their inclusion and
individual global sources are managed together in the desktop Backup settings.

The snapshot uses one directory per category:

```text
skills/
mcp/
rules/
hooks/
manifest.json
```

It contains a portable manifest and the selected files; it never writes Tendi
settings or credentials into that repository.

Connect a remote from the desktop **Backup** page, or use the CLI:

```sh
tendi skills backup configure git@github.com:you/tendi-skills-backup.git --device "Work Mac"
tendi skills backup run
tendi skills backup versions
```

The system Git credential helper handles authentication. HTTP(S) remote URLs
with embedded credentials are rejected, so configure an SSH remote or a
credential-free HTTPS URL instead. GitHub shorthands such as `owner/repository`
and `github.com/owner/repository` are expanded to HTTPS remotes automatically.
The remote is checked with Git when backup setup is saved.

## Inclusion rules

Only global managed skills with a Tendi source record are included. MCP, rules,
and hooks are backed up from their global source files. Project-scoped content
is not included.

New global items are included automatically when their category is enabled.
Individual items can be excluded in **Settings → Developer → Backup**.

Skills inside any Git worktree are always excluded. Dependency/cache folders,
`.git`, symlinks, oversized skills (over 100 MiB), and skills containing common
secret filenames or token/private-key patterns are excluded as well. A backup
is intentionally all-or-nothing per skill.

## Restore

Select a backup version in the desktop page. The preview lists each selected
skill and global configuration source before anything is written. Existing
targets require an explicit choice: keep the existing content, replace it, or
keep both.

The CLI supports restoring a whole version or individual IDs:

```sh
tendi skills backup restore <commit> --to shared --scope global --dry-run
tendi skills backup restore <commit> --skill review --to codex --scope global --conflict keep-both --yes
```

Restored skills become locally managed backup sources and are included in later
snapshots. Git history is append-only; Tendi does not prune old backup commits.

Concurrent device edits are merged by skill content: identical copies collapse,
while different contents stay as separate backup entries. If the checkout was
manually changed into an unresolved Git conflict, affected skills show **Needs
attention**. Resolve that Git conflict in the checkout directory shown on the
Backup page, then run a backup again.
