# Agent Operating Instructions

## General

- Use concise English by default. Lead with the conclusion, then provide evidence.
- Before editing code, inspect the relevant code, existing components, styles, tests, and Git worktree state.
- Preserve the user's existing changes. Do not overwrite, reset, or include unrelated changes.
- Fix root causes and follow the existing architecture. Do not add fallbacks, compatibility layers, or duplicate abstractions without evidence.
- At the end, report design changes and their reasons, added/deleted lines by feature, unfinished work, and verification results.
- Keep UI labels concise; do not add descriptive copy unless it is necessary for comprehension or action.

## Shared UI Components

- Before adding UI, search `apps/desktop/src/components/shared` and existing page usage.
- Reuse an existing shared component when it expresses the same semantics. Do not duplicate JSX, state handling, or styles in pages or `lib`.
- If a shared component cannot express the scenario, document the difference first. Extend the shared component and reuse it instead of creating a parallel implementation.
- States with the same visual semantics must use the same component and CSS rules. Keep separate components only when behavior, Radix semantics, or layout are demonstrably different.

## Loading UI

- Use `LoadingIcon` for loading icons inside buttons.
- Use `LoadingInline` when an icon and text are needed. Do not hand-write `RefreshCw` with a separate spinner class in icon, action, or editor buttons.
- Do not add `loadingSpinner`, `dialogLoadingIcon`, `skillRefreshSpinning`, `editorSaveSpinner`, or equivalent duplicate animation classes. Pass sizes through the `LoadingIcon` `size` prop.
- Loading buttons must correctly handle `disabled`, `aria-busy`, icon alignment, and existing success/error states.

## Verification

- After editing, search the relevant files to confirm that duplicate implementations, obsolete CSS classes, and unused imports are removed.
- Run targeted tests, type checks, or builds appropriate to the impact. Explain when a check cannot be run.
- Do not hide product behavior problems with test, mock, hook, or CSS special cases.
