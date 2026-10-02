#!/usr/bin/env bash
# Build four independently deployable LedgerZero component artifacts.
set -euo pipefail

cd "$(dirname "$0")/.."

LOCKED_BUILD=false
case "${1:-}" in
  --locked) LOCKED_BUILD=true; shift ;;
  "") ;;
  *) echo "usage: $0 [--locked]" >&2; exit 1 ;;
esac
[[ $# -eq 0 ]] || { echo "usage: $0 [--locked]" >&2; exit 1; }

if [[ "$LOCKED_BUILD" == false ]]; then
  bash ./scripts/upgrade-deps.sh
fi

if ! command -v cargo >/dev/null 2>&1; then
  [[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
  [[ -x /opt/homebrew/opt/rustup/bin/cargo ]] && PATH="/opt/homebrew/opt/rustup/bin:$PATH"
fi
command -v cargo >/dev/null 2>&1 || { echo "cargo not found — install Rust: https://rustup.rs" >&2; exit 1; }

VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
STAMP=$(date +%Y%m%d%H%M%S)
REVISION=$(git rev-parse HEAD 2>/dev/null || printf unknown)
if git diff --quiet --ignore-submodules HEAD -- 2>/dev/null && \
   [[ -z "$(git ls-files --others --exclude-standard 2>/dev/null)" ]]; then
  GIT_STATE=clean
else
  GIT_STATE=dirty
fi
PLATFORM="$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)"
RUST_ABI="$(rustc -vV | awk '/^release:|^commit-hash:/ { printf "%s%s", separator, $2; separator="-" }')"
OUTPUT_DIR="dist/components/${VERSION}-${STAMP}"

echo "== Building engine and backend =="
if [[ "$LOCKED_BUILD" == true ]]; then
  RUSTFLAGS="${RUSTFLAGS:-} -C prefer-dynamic" cargo build --locked --release -p ledgerzero-backend
else
  RUSTFLAGS="${RUSTFLAGS:-} -C prefer-dynamic" cargo build --release -p ledgerzero-backend
fi

echo "== Building launcher =="
(cd frontend && npm run build)

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

write_manifest() {
  local root="$1" component="$2" payload_hash="$3"
  shift 3
  {
    printf 'manifest_format=1\n'
    printf 'component=%s\n' "$component"
    printf 'component_version=%s\n' "$VERSION"
    printf 'payload_sha256=%s\n' "$payload_hash"
    printf 'source_revision=%s\n' "$REVISION"
    printf 'source_state=%s\n' "$GIT_STATE"
    printf 'platform=%s\n' "$PLATFORM"
    printf 'packaged_at=%s\n' "$STAMP"
    printf '%s\n' "$@"
  } >"$root/component.manifest"
}

archive_component() {
  local component="$1" root="$2" archive
  archive="$OUTPUT_DIR/ledgerzero-${component}-${VERSION}-${STAMP}-${PLATFORM}.tar.gz"
  tar -czf "$archive" -C "$(dirname "$root")" "$(basename "$root")"
  printf 'Artifact[%s]: %s\n' "$component" "$archive"
}

ENGINE_ROOT="$OUTPUT_DIR/ledgerzero-engine-${VERSION}"
mkdir -p "$ENGINE_ROOT/lib"
case "$(uname -s)" in
  Darwin)
    cp target/release/libledgerzero_engine.dylib "$ENGINE_ROOT/lib/"
    RUST_HOST=$(rustc -vV | sed -n 's/^host: //p')
    RUST_SYSROOT=$(rustc --print sysroot)
    cp "$RUST_SYSROOT/lib/rustlib/$RUST_HOST/lib/"libstd-*.dylib "$ENGINE_ROOT/lib/"
    install_name_tool -id '@rpath/libledgerzero_engine.dylib' "$ENGINE_ROOT/lib/libledgerzero_engine.dylib"
    ;;
  Linux)
    cp target/release/libledgerzero_engine.so "$ENGINE_ROOT/lib/"
    RUST_HOST=$(rustc -vV | sed -n 's/^host: //p')
    RUST_SYSROOT=$(rustc --print sysroot)
    cp "$RUST_SYSROOT/lib/rustlib/$RUST_HOST/lib/"libstd-*.so "$ENGINE_ROOT/lib/"
    ;;
  *) echo "unsupported packaging platform: $(uname -s)" >&2; exit 1 ;;
esac
ENGINE_HASH=$(find "$ENGINE_ROOT/lib" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}')
write_manifest "$ENGINE_ROOT" engine "$ENGINE_HASH" 'engine_api=1' 'storage_format=1' "rust_abi=$RUST_ABI"
archive_component engine "$ENGINE_ROOT"

BACKEND_ROOT="$OUTPUT_DIR/ledgerzero-backend-${VERSION}"
mkdir -p "$BACKEND_ROOT"
cp target/release/ledgerzero-backend "$BACKEND_ROOT/"
if [[ "$(uname -s)" == Darwin ]]; then
  ENGINE_LINK=$(otool -L "$BACKEND_ROOT/ledgerzero-backend" | awk '/libledgerzero_engine\.dylib/ { print $1; exit }')
  [[ -n "$ENGINE_LINK" ]] || { echo "backend is not dynamically linked to the engine" >&2; exit 1; }
  install_name_tool -change "$ENGINE_LINK" '@rpath/libledgerzero_engine.dylib' "$BACKEND_ROOT/ledgerzero-backend"
  if ! otool -l "$BACKEND_ROOT/ledgerzero-backend" | grep -q '@executable_path/../engine/lib'; then
    install_name_tool -add_rpath '@executable_path/../engine/lib' "$BACKEND_ROOT/ledgerzero-backend"
  fi
fi
cp server.config.example.toml "$BACKEND_ROOT/"
cp docs/LedgerZero_Run_and_Deploy.md "$BACKEND_ROOT/DEPLOY.md"
BACKEND_HASH=$(shasum -a 256 "$BACKEND_ROOT/ledgerzero-backend" | awk '{print $1}')
write_manifest "$BACKEND_ROOT" backend "$BACKEND_HASH" 'backend_api=2' 'backend_api_compat_min=1' 'requires_engine_api=1' 'requires_storage_format=1' "requires_rust_abi=$RUST_ABI"
archive_component backend "$BACKEND_ROOT"

LAUNCHER_ROOT="$OUTPUT_DIR/ledgerzero-launcher-${VERSION}"
mkdir -p "$LAUNCHER_ROOT"
cp -R frontend/dist "$LAUNCHER_ROOT/dist"
LAUNCHER_HASH=$(find "$LAUNCHER_ROOT/dist" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}')
write_manifest "$LAUNCHER_ROOT" launcher "$LAUNCHER_HASH" 'requires_backend_api=2'
archive_component launcher "$LAUNCHER_ROOT"

RUNTIME_ROOT="$OUTPUT_DIR/ledgerzero-runtime-frontends-${VERSION}"
mkdir -p "$RUNTIME_ROOT/workflows"
if [[ -d dev_artifacts/workflows ]]; then
  cp -R dev_artifacts/workflows/. "$RUNTIME_ROOT/workflows/"
fi
RUNTIME_HASH=$(find "$RUNTIME_ROOT/workflows" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}')
write_manifest "$RUNTIME_ROOT" runtime-frontends "$RUNTIME_HASH" 'runtime_frontend_api=1' 'requires_backend_api=1'
archive_component runtime-frontends "$RUNTIME_ROOT"

cat >"$OUTPUT_DIR/component-inventory.txt" <<EOF
inventory_format=1
release_version=$VERSION
source_revision=$REVISION
source_state=$GIT_STATE
platform=$PLATFORM
engine_api=1
backend_api=2
storage_format=1
runtime_frontend_api=1
rust_abi=$RUST_ABI
EOF

echo "Inventory: $OUTPUT_DIR/component-inventory.txt"
