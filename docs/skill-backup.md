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

## Sync and pull

**Sync** publishes this device's selected current set to the repository. Updating
an included skill replaces its live version. Deleting or excluding it removes it
from the live set. Earlier sets remain available in Git history.

**Pull** fetches that published set and applies it to this device. The desktop
installs skills in the global shared directory; the CLI can select another target. Subsequent pulls
update and remove the items this device previously received from this repository.
Local items that were never pulled remain local. An existing local item with
conflicting content requires a choice on the first pull: replace or keep local.
Skill and rule conflicts also support keeping both. Once resolved, later pulls
use the recorded destination.

These are explicit actions. Connecting a repository, focusing the desktop, and
local edits do not publish automatically. A typical setup is to maintain the set
on device A, click **Sync**, then click **Pull** on devices B and C.

The CLI supports the same two directions:

```sh
tendi skills sync run
tendi skills sync pull --to shared --scope global --dry-run
tendi skills sync pull --to shared --scope global --yes
```

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

An explicit sync publishes the calling device's selected set as the current
set, including when another device previously published different content. The
Git history retains previous commits; older skill versions are not added to the
current manifest. A receiving device should pull to follow the maintained set.
If the checkout contains an unresolved Git conflict, resolve it before syncing
or pulling again.
