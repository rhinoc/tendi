# Sync

Tendi can snapshot global managed skills, MCP, rules, and hooks into a Git
repository. All four categories are included by default. Their
inclusion and individual global sources are managed together in the desktop
Sync settings.

Desktop Sync settings accept either a Git remote URL or a local repository path.
When a local path is selected, Tendi creates and initializes it if needed, then
pushes only when an `origin` remote is available; otherwise it keeps the commit
local.

The snapshot uses one directory per category:

```text
skills/
mcp/
rules/
hooks/
manifest.json
```

It contains a portable manifest and the selected content; it never writes Tendi
settings or credentials into that repository.

Configure a remote or local checkout from the desktop **Sync** page. The CLI
supports remote configuration:

```sh
tendi skills sync configure git@github.com:you/tendi-skills-backup.git
tendi skills sync run
tendi skills sync versions
```

The system Git credential helper handles authentication. HTTP(S) remote URLs
with embedded credentials are rejected, so configure an SSH remote or a
credential-free HTTPS URL instead. GitHub shorthands such as `owner/repository`
and `github.com/owner/repository` are expanded to HTTPS remotes automatically.
The remote is checked with Git when sync setup is saved.

## Automatic snapshots

When the daemon starts, it marks one initial snapshot as pending. Writes to
managed Skills, MCP, rules, or hooks only mark a snapshot as dirty. Every ten
minutes, Tendi creates one sync snapshot if the dirty flag is set; multiple writes in
that interval are combined. A sync failure keeps the dirty flag set for the
next interval. The desktop **Sync now** action still runs immediately.

## Inclusion rules

Only global managed skills with a Tendi source record are included. The other
categories use their own item boundaries: each MCP server, each hook, and each
Rules file is synced separately. MCP and hook
snapshots contain only the selected entry, not the complete source configuration
file.
Project-scoped configuration files are not included.

Plugins, cloud/team-managed rules, managed/admin-only files, and provider
account state are not local portable files and are intentionally not included.

New global items are included automatically when their category is enabled.
Individual items can be excluded in **Settings → Developer → Sync**.

Skills inside any Git worktree are always excluded. Dependency/cache folders,
`.git`, symlinks, oversized skills (over 100 MiB), and skills containing common
secret filenames or token/private-key patterns are excluded as well. A sync
is intentionally all-or-nothing per skill.

## Restore

Select a sync version in the desktop page. The preview lists each selected
skill, MCP server, hook, Rules file, or agent file before anything is written.
MCP and hook restores merge the selected entry into the current provider
configuration and keep other entries. Rules and agent-file restores replace the
selected file as a whole.
Existing skill and Rules-file targets require an explicit choice: keep the
existing content, replace it, or keep both.

The CLI supports restoring a whole version or individual IDs:

```sh
tendi skills sync restore <commit> --to shared --scope global --dry-run
tendi skills sync restore <commit> --skill review --to codex --scope global --conflict keep-both --yes
```

Restored skills become locally managed sync sources and are included in later
snapshots. Git history is append-only; Tendi does not prune old sync commits.

Concurrent device edits are merged by skill content: identical copies collapse,
while different contents stay as separate sync entries. If the checkout was
manually changed into an unresolved Git conflict, affected skills show **Needs
attention**. Resolve that Git conflict in the checkout directory shown on the
Sync page, then run a sync again.
