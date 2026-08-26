# Tendi Agent Instructions

These are Tendi-specific conventions. Follow higher-level agent instructions separately.

## UI Design and Layout

- Treat the existing product design system as the source of truth. Find the closest page and component before changing UI, then follow its information architecture, layout, spacing, typography, colors, controls, and interactions.
- Reusing existing styling means preserving visual and structural consistency. Do not introduce a new visual language, decorative hierarchy, or component variant unless explicitly required or the existing system lacks the needed capability.
- Make the smallest necessary change. Do not redesign unrequested information architecture, copy, behavior, or visual hierarchy.
- Base responsive behavior on actual available space and content measurements. Do not replace layout logic with fixed counts or guesses.

## UI Copy

- Keep UI copy concise, accurate, and actionable. Match the copy density of nearby pages.
- Do not add descriptions, warnings, confirmation text, or decorative headings unless they help the user understand or complete an action.
- If the user asks to change one sentence, change only that sentence.

## Shared UI Components

- Before adding UI, search `apps/desktop/src/components/shared` and existing page usage.
- Reuse a shared component when it expresses the same semantics. Do not duplicate JSX, state handling, or styles in pages or `lib`.
- If a shared component cannot express the scenario, document the difference first, then extend the shared component instead of creating a parallel implementation.
- States with the same visual semantics must use the same component and CSS rules. Keep separate components only when behavior, Radix semantics, or layout are demonstrably different.

## Provider-Owned Architecture

- Provider-specific behavior belongs to the owning provider. Configuration discovery, parsing, status semantics, mutation, read-only rules, and path resolution for Codex, Claude Code, Cursor, and other agents must be implemented through the provider trait and the corresponding `crates/tendi-core/src/providers/{codex,claude,cursor}.rs` implementation.
- Shared modules may provide orchestration and format-level utilities, but must not infer provider behavior from a file extension or silently apply one provider's semantics to all providers.
- Before changing a shared MCP, hooks, config, or UI path, inspect every affected provider implementation and its tests. Carry provider identity through commands and data models when behavior differs by provider.
- Do not implement a UI-only switch, fallback, or generic write-back path and present it as provider support. A provider feature is complete only when the provider owner, command boundary, persistence/refresh path, and user-facing control agree on the same behavior.

## Change Scope and Worktree Hygiene

- Inspect `git status` and the relevant diff before editing. Preserve existing user changes and do not reformat, rename, migrate, or refactor unrelated files.
- Make the smallest end-to-end change at the correct owner. Do not scatter duplicated logic across pages, shared helpers, daemon dispatch, and providers just to make a control render.
- When a requested feature crosses providers, list the provider-specific behavior and verification entry points before implementation. Unsupported providers must remain explicitly unsupported instead of receiving guessed generic behavior.

## Session Title Formatting

- Any chart or graph that displays a session title or label must pass it through `formatSessionTitle` from `apps/desktop/src/lib/session-preview.ts`.
- Use `TranscriptLinkText` for regular HTML session-title previews. Do not duplicate image or URL parsing in a page or chart.

## Loading UI

- Use `LoadingIcon` for loading icons inside buttons.
- Use `LoadingInline` when an icon and text are needed. Do not hand-write `RefreshCw` with a separate spinner class.
- Do not add duplicate animation classes such as `loadingSpinner`, `dialogLoadingIcon`, `skillRefreshSpinning`, or `editorSaveSpinner`. Pass sizes through `LoadingIcon`.
- Loading buttons must handle `disabled`, `aria-busy`, icon alignment, and existing success/error states correctly.
- Keep idle, loading, success, and error states geometrically stable. Use a stable width when labels differ, center the contents, and do not size from a transient loading label.
- For button actions, show only `LoadingIcon` while loading. Put the description in `aria-label` and expose the state with `aria-busy`; do not add visible `Loading…` or `Saving…` text.
- Use concise labels that describe the actual operation. Use `Switch`, not `Activate`, when switching the active profile.

## Verification

- After editing, check relevant files for duplicate implementations, obsolete CSS classes, and unused imports.
- Run targeted tests, type checks, or builds appropriate to the change.
- Explain any check that cannot be run.
- Do not hide product behavior problems with test, mock, hook, or CSS special cases.
