# Skill backup

Tendi can snapshot managed, non-project skills into a Git repository. The backup
contains a portable manifest and the complete skill files; it never writes
absolute local paths, Tendi settings, or credentials into that repository.

Connect a remote from the desktop **Backup** page, or use the CLI:

```sh
tendi skills backup configure git@github.com:you/tendi-skills-backup.git --device "Work Mac"
tendi skills backup run
tendi skills backup versions
```

The system Git credential helper handles authentication. HTTP(S) remote URLs
with embedded credentials are rejected, so configure an SSH remote or a
credential-free HTTPS URL instead.

## Inclusion rules

Only skills with a Tendi source record are included. Use the **Add** button in
the Skills table, or `tendi skills backup add <path>`, to include a local skill.

Skills inside any Git worktree are always excluded. Dependency/cache folders,
`.git`, symlinks, oversized skills (over 100 MiB), and skills containing common
secret filenames or token/private-key patterns are excluded as well. A backup
is intentionally all-or-nothing per skill.

## Restore

Select a backup version in the desktop page, choose individual skills, then
choose an agent and scope. The preview lists each destination before anything
is written. Existing directories require an explicit choice: keep the existing
skill, replace it, or keep both.

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
