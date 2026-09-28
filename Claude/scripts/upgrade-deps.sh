#!/usr/bin/env bash
# Local builds optimistically take the newest published dependencies.
# Review the resulting manifest and lockfile diff before committing it.
set -euo pipefail

cd "$(dirname "$0")/.."
TOOLS_DIR="$(pwd)/.local-build-tools"

if ! command -v cargo >/dev/null 2>&1; then
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  [ -x /opt/homebrew/opt/rustup/bin/cargo ] && PATH="/opt/homebrew/opt/rustup/bin:$PATH"
fi

for tool in cargo npm python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing $tool; install it before running a local build." >&2
    exit 1
  fi
done

if ! command -v uv >/dev/null 2>&1; then
  if [ ! -x "$TOOLS_DIR/python/bin/uv" ]; then
    python3 -m venv "$TOOLS_DIR/python"
    "$TOOLS_DIR/python/bin/pip" install uv
  fi
  PATH="$TOOLS_DIR/python/bin:$PATH"
fi
if ! cargo upgrade --help >/dev/null 2>&1; then
  if [ ! -x "$TOOLS_DIR/cargo/bin/cargo-upgrade" ]; then
    cargo install cargo-edit --locked --root "$TOOLS_DIR/cargo"
  fi
  PATH="$TOOLS_DIR/cargo/bin:$PATH"
fi

echo "== Upgrade Rust dependencies =="
cargo upgrade --incompatible allow --pinned allow --exclude rand
cargo update

echo "== Upgrade frontend dependencies =="
(
  cd frontend
  npm exec --yes --package=npm-check-updates -- ncu --upgrade --target latest --reject react,react-dom
  npm exec --yes --package=npm-check-updates -- ncu --upgrade --target minor --filter react,react-dom
  npm install --no-audit --no-fund
)

echo "== Upgrade Python MCP dependencies =="
(
  cd mcp_server
  uv lock --upgrade
  uv sync --locked
)

echo "Dependency manifests and lockfiles are ready for review before commit."
