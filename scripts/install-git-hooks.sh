#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if ! command -v pre-commit >/dev/null 2>&1; then
  echo "pre-commit is required to install git hooks." >&2
  echo "Install it with: brew install pre-commit   # or: pipx install pre-commit" >&2
  exit 1
fi

chmod +x "$root/.githooks/pre-commit"
git config core.hooksPath "$root/.githooks"
pre-commit install-hooks
echo "Installed repo git hooks from $root/.githooks"
