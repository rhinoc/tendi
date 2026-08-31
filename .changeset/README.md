# Release changesets

Every user-visible release must include at least one changeset file in this
directory. Use a short, user-facing English sentence and one of these types:

- `added`: New user-visible functionality.
- `changed`: Improvements to existing behavior.
- `fixed`: Bug fixes.

Example:

```markdown
---
type: fixed
---

Keep the update dialog open while release notes are loading.
```

Requirements:

- Use one non-empty sentence per file.
- Write the sentence in ASCII English.
- Do not add a Markdown list marker; the release script adds it.
- Name the file after the change, for example `show-update-notes.md`.

The release workflow groups pending changesets into Markdown release notes and
publishes the same notes to the Tauri updater manifest and GitHub Releases.
