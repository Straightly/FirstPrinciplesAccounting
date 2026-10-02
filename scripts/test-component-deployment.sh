#!/usr/bin/env bash
# Isolated L3 acceptance drill. It never touches the persistent staging install.
set -euo pipefail

cd "$(dirname "$0")/.."
STAGE_ROOT="$(mktemp -d /tmp/fpa-l3-acceptance.XXXXXX)"
export FPA_STAGE_ROOT="$STAGE_ROOT"
export FPA_STAGE_ADDR="${FPA_L3_TEST_ADDR:-127.0.0.1:18081}"

cleanup() {
  ./scripts/local-staging.sh stop >/dev/null 2>&1 || true
  rm -rf "$STAGE_ROOT"
}
trap cleanup EXIT

output="$(mktemp /tmp/fpa-l3-package.XXXXXX)"
trap 'rm -f "$output"; cleanup' EXIT
./scripts/package.sh --locked | tee "$output"

artifact() { sed -n "s/^Artifact\[$1\]: //p" "$output" | tail -1; }
fingerprint() {
  find "$STAGE_ROOT/components/$1" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}'
}

variant_artifact() {
  local component="$1" work top result
  work="$(mktemp -d /tmp/fpa-l3-variant.XXXXXX)"
  tar -xzf "$(artifact "$component")" -C "$work"
  top="$(find "$work" -mindepth 1 -maxdepth 1 -type d -print -quit)"
  sed -i '' "s/^component_version=.*/component_version=0.1.0-${component}-drill/" "$top/component.manifest"
  result="$work/${component}-drill.tar.gz"
  tar -czf "$result" -C "$work" "$(basename "$top")"
  echo "$result"
}

for component in engine backend launcher runtime-frontends; do
  ./scripts/local-staging.sh deploy-component "$component" "$(artifact "$component")"
done
./scripts/local-staging.sh status >/dev/null

mkdir -p "$STAGE_ROOT/data/books/l3-proof"
echo preserved >"$STAGE_ROOT/data/books/l3-proof/sentinel"

for changed in engine backend launcher runtime-frontends; do
  before_engine="$(fingerprint engine)"
  before_backend="$(fingerprint backend)"
  before_launcher="$(fingerprint launcher)"
  before_runtime_frontends="$(fingerprint runtime-frontends)"
  candidate="$(variant_artifact "$changed")"
  ./scripts/local-staging.sh deploy-component "$changed" "$candidate" >/dev/null
  rm -rf "$(dirname "$candidate")"
  case "$changed" in
    engine) old_changed="$before_engine" ;;
    backend) old_changed="$before_backend" ;;
    launcher) old_changed="$before_launcher" ;;
    runtime-frontends) old_changed="$before_runtime_frontends" ;;
  esac
  [[ "$old_changed" != "$(fingerprint "$changed")" ]] || { echo "$changed replacement did not change its installed version" >&2; exit 1; }
  for component in engine backend launcher runtime-frontends; do
    [[ "$component" == "$changed" ]] && continue
    variable="before_${component//-/_}"
    [[ "${!variable}" == "$(fingerprint "$component")" ]] || { echo "$changed replacement altered $component" >&2; exit 1; }
  done
  [[ "$(cat "$STAGE_ROOT/data/books/l3-proof/sentinel")" == preserved ]] || { echo "durable data changed" >&2; exit 1; }
done

bad_root="$(mktemp -d /tmp/fpa-l3-incompatible.XXXXXX)"
tar -xzf "$(artifact launcher)" -C "$bad_root"
bad_top="$(find "$bad_root" -mindepth 1 -maxdepth 1 -type d -print -quit)"
sed -i '' 's/^requires_backend_api=.*/requires_backend_api=999/' "$bad_top/component.manifest"
bad_artifact="$bad_root/incompatible-launcher.tar.gz"
tar -czf "$bad_artifact" -C "$bad_root" "$(basename "$bad_top")"
if ./scripts/local-staging.sh deploy-component launcher "$bad_artifact" >/dev/null 2>&1; then
  echo "incompatible launcher was accepted" >&2
  exit 1
fi
rm -rf "$bad_root"

bad_backend_root="$(mktemp -d /tmp/fpa-l3-bad-backend.XXXXXX)"
tar -xzf "$(artifact backend)" -C "$bad_backend_root"
bad_backend_top="$(find "$bad_backend_root" -mindepth 1 -maxdepth 1 -type d -print -quit)"
cp /usr/bin/false "$bad_backend_top/ledgerzero-backend"
bad_backend_hash="$(shasum -a 256 "$bad_backend_top/ledgerzero-backend" | awk '{print $1}')"
sed -i '' "s/^payload_sha256=.*/payload_sha256=$bad_backend_hash/; s/^component_version=.*/component_version=failed-health-drill/" "$bad_backend_top/component.manifest"
bad_backend_artifact="$bad_backend_root/failed-health-backend.tar.gz"
tar -czf "$bad_backend_artifact" -C "$bad_backend_root" "$(basename "$bad_backend_top")"
if ./scripts/local-staging.sh deploy-component backend "$bad_backend_artifact" >/dev/null 2>&1; then
  echo "backend that fails startup health was accepted" >&2
  exit 1
fi
./scripts/local-staging.sh redeploy-component backend >/dev/null
rm -rf "$bad_backend_root"
[[ "$(cat "$STAGE_ROOT/data/books/l3-proof/sentinel")" == preserved ]] || { echo "failed-health recovery changed durable data" >&2; exit 1; }

before_launcher="$(fingerprint launcher)"
before_runtime_frontends="$(fingerprint runtime-frontends)"
./scripts/local-staging.sh deploy-engine-backend "$(artifact engine)" "$(artifact backend)" >/dev/null
[[ "$before_launcher" == "$(fingerprint launcher)" ]] || { echo "coordinated engine/backend deployment altered launcher" >&2; exit 1; }
[[ "$before_runtime_frontends" == "$(fingerprint runtime-frontends)" ]] || { echo "coordinated engine/backend deployment altered runtime frontends" >&2; exit 1; }
[[ "$(cat "$STAGE_ROOT/data/books/l3-proof/sentinel")" == preserved ]] || { echo "coordinated engine/backend deployment changed durable data" >&2; exit 1; }

./scripts/local-staging.sh recover >/dev/null
[[ "$(cat "$STAGE_ROOT/data/books/l3-proof/sentinel")" == preserved ]] || { echo "recovery changed durable data" >&2; exit 1; }
./scripts/local-staging.sh status >/dev/null

echo "L3 component deployment acceptance passed: four changed-version replacements, coordinated engine/backend API-boundary replacement, incompatibility rejection, failed-health recovery, and delete/redeploy recovery."
