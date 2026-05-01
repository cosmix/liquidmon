#!/usr/bin/env bash
# Wire repository-tracked git hooks into this clone by pointing core.hooksPath
# at .githooks/. Idempotent — re-running just refreshes the +x bits.

set -euo pipefail

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "install.sh: not inside a git work tree" >&2
    exit 1
fi

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

git config core.hooksPath .githooks
chmod +x .githooks/pre-commit .githooks/pre-push

echo "Installed git hooks from .githooks/"
echo "  pre-commit  → cargo fmt + clippy on staged Rust changes"
echo "  pre-push    → cargo test + cargo audit (if installed) + tag/version match"
echo ""
echo "Optional: install 'cargo audit' for dependency CVE checks:"
echo "  cargo install cargo-audit --locked"
