#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

rust_sources=()
while IFS= read -r file; do
  rust_sources+=("$file")
done < <(
  find "$repo_root/crates" "$repo_root/apps/desktop/src-tauri" \
    -type f -name '*.rs' \
    ! -path '*/tests/*' \
    ! -name '*_tests.rs' \
    -print
)

if ((${#rust_sources[@]} == 0)); then
  exit 0
fi

violations="$(rg -n \
  '#\[(test|tokio::test)\]|^[[:space:]]*mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*(tests|_tests)[[:space:]]*\{' \
  "${rust_sources[@]}" || true)"

if [[ -n "$violations" ]]; then
  echo "Rust tests must live in *_tests.rs files or crate tests/ directories." >&2
  echo "$violations" >&2
  exit 1
fi
