#!/usr/bin/env bash
# Build, deploy, and operate an isolated component-based FPA staging install.
set -euo pipefail

SCRIPT_PATH="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")"
SCRIPT_DIR="$(dirname "$SCRIPT_PATH")"
SOURCE_CANDIDATE="$(cd "$SCRIPT_DIR/.." 2>/dev/null && pwd -P || true)"
if [[ -f "$SOURCE_CANDIDATE/Cargo.toml" && -x "$SOURCE_CANDIDATE/scripts/package.sh" ]]; then
  SOURCE_ROOT="$SOURCE_CANDIDATE"
else
  SOURCE_ROOT=""
fi

STAGE_ROOT="${FPA_STAGE_ROOT:-${HOME}/Deployments/FPA-Staging}"
DEFAULT_LISTEN_ADDR="${FPA_STAGE_ADDR:-127.0.0.1:8081}"
COMPONENTS=(engine backend launcher runtime-frontends)

die() { echo "Error: $*" >&2; exit 1; }

usage() {
  cat <<'EOF'
Usage: fpa-stage <command> [arguments]

  build-deploy                         Build/deploy all four components.
  deploy-component <name> <artifact>  Replace only one component.
  redeploy-component <name>           Reinstall its recorded artifact.
  recover                              Delete/redeploy all components from records.
  start | stop | restart               Operate the staging backend.
  status                               Show health, paths, and component inventory.
  logs                                 Follow the backend log.
  config                               Print the persistent config path.

Components: engine, backend, launcher, runtime-frontends.
Configuration, books, backups, logs, artifacts, and controls are never removed
by component deployment or recovery.
EOF
}

validate_stage_root() {
  case "$STAGE_ROOT" in "" | "/" | "$HOME") die "unsafe FPA_STAGE_ROOT: $STAGE_ROOT" ;; esac
  [[ -z "$SOURCE_ROOT" || "$STAGE_ROOT" != "$SOURCE_ROOT" ]] || die "staging cannot be the source repository"
  [[ ! -L "$STAGE_ROOT" ]] || die "staging root must not be a symbolic link"
}

set_paths() {
  COMPONENTS_DIR="$STAGE_ROOT/components"
  ENGINE_DIR="$COMPONENTS_DIR/engine"
  BACKEND_DIR="$COMPONENTS_DIR/backend"
  LAUNCHER_DIR="$COMPONENTS_DIR/launcher"
  RUNTIME_DIR="$COMPONENTS_DIR/runtime-frontends"
  CONFIG_DIR="$STAGE_ROOT/config"
  CONFIG_FILE="$CONFIG_DIR/server.config.toml"
  DATA_DIR="$STAGE_ROOT/data"
  BOOKS_DIR="$DATA_DIR/books"
  BACKUPS_DIR="$DATA_DIR/backups"
  LOG_DIR="$STAGE_ROOT/logs"
  APP_LOG="$LOG_DIR/application.log"
  ARTIFACTS_DIR="$STAGE_ROOT/artifacts"
  DEPLOY_DIR="$STAGE_ROOT/deploy"
  RUN_DIR="$STAGE_ROOT/run"
  PID_FILE="$RUN_DIR/fpa.pid"
  BINARY="$BACKEND_DIR/ledgerzero-backend"
  LEGACY_BINARY="$STAGE_ROOT/app/ledgerzero-backend"
  LAUNCH_LABEL="com.nothingbuttrust.fpa-staging.$(printf '%s' "$STAGE_ROOT" | shasum -a 256 | cut -c1-12)"
}

component_dir() {
  case "$1" in
    engine) echo "$ENGINE_DIR" ;;
    backend) echo "$BACKEND_DIR" ;;
    launcher) echo "$LAUNCHER_DIR" ;;
    runtime-frontends) echo "$RUNTIME_DIR" ;;
    *) die "unknown component: $1" ;;
  esac
}

ensure_layout() {
  validate_stage_root
  mkdir -p "$STAGE_ROOT"
  STAGE_ROOT="$(cd "$STAGE_ROOT" && pwd -P)"
  set_paths
  local path
  for path in "$COMPONENTS_DIR" "$CONFIG_DIR" "$DATA_DIR" "$LOG_DIR" "$ARTIFACTS_DIR" "$DEPLOY_DIR" "$RUN_DIR"; do
    [[ ! -L "$path" ]] || die "managed path must not be a symbolic link: $path"
  done
  mkdir -p "$COMPONENTS_DIR" "$CONFIG_DIR" "$BOOKS_DIR" "$BACKUPS_DIR" "$LOG_DIR" "$ARTIFACTS_DIR" "$DEPLOY_DIR" "$RUN_DIR"
}

manifest_value() {
  local file="$1" key="$2"
  sed -n "s/^${key}=//p" "$file" | head -1
}

require_manifest_key() {
  local file="$1" key="$2"
  [[ -n "$(manifest_value "$file" "$key")" ]] || die "manifest is missing $key"
}

require_equal() {
  local left="$1" right="$2" message="$3"
  [[ -z "$left" || -z "$right" || "$left" == "$right" ]] || die "$message ($left != $right)"
}

check_compatibility() {
  local candidate_component="${1:-}" candidate_manifest="${2:-}" engine_manifest backend_manifest launcher_manifest runtime_manifest
  engine_manifest="$ENGINE_DIR/component.manifest"
  backend_manifest="$BACKEND_DIR/component.manifest"
  launcher_manifest="$LAUNCHER_DIR/component.manifest"
  runtime_manifest="$RUNTIME_DIR/component.manifest"
  [[ "$candidate_component" == engine ]] && engine_manifest="$candidate_manifest"
  [[ "$candidate_component" == backend ]] && backend_manifest="$candidate_manifest"
  [[ "$candidate_component" == launcher ]] && launcher_manifest="$candidate_manifest"
  [[ "$candidate_component" == runtime-frontends ]] && runtime_manifest="$candidate_manifest"

  if [[ -f "$engine_manifest" && -f "$backend_manifest" ]]; then
    require_equal "$(manifest_value "$engine_manifest" engine_api)" "$(manifest_value "$backend_manifest" requires_engine_api)" "incompatible engine API"
    require_equal "$(manifest_value "$engine_manifest" storage_format)" "$(manifest_value "$backend_manifest" requires_storage_format)" "incompatible storage format"
    require_equal "$(manifest_value "$engine_manifest" rust_abi)" "$(manifest_value "$backend_manifest" requires_rust_abi)" "incompatible Rust dynamic ABI"
  fi
  if [[ -f "$backend_manifest" && -f "$launcher_manifest" ]]; then
    require_equal "$(manifest_value "$backend_manifest" backend_api)" "$(manifest_value "$launcher_manifest" requires_backend_api)" "incompatible launcher/backend API"
  fi
  if [[ -f "$backend_manifest" && -f "$runtime_manifest" ]]; then
    require_equal "$(manifest_value "$backend_manifest" backend_api)" "$(manifest_value "$runtime_manifest" requires_backend_api)" "incompatible runtime-frontend/backend API"
  fi
}

read_listen_addr() { awk -F '"' '/^[[:space:]]*listen_addr[[:space:]]*=/ { print $2; exit }' "$CONFIG_FILE"; }
health_url() { local addr; addr="$(read_listen_addr)"; [[ -n "$addr" ]] || die "listen_addr is missing"; echo "http://${addr}/api/health"; }
pid_is_running() { [[ -f "$PID_FILE" ]] && local p="$(cat "$PID_FILE")" && [[ "$p" =~ ^[0-9]+$ ]] && kill -0 "$p" 2>/dev/null; }

verified_pid() {
  local pid command
  pid="$(cat "$PID_FILE")"
  [[ "$pid" =~ ^[0-9]+$ ]] || die "invalid PID file"
  command="$(ps -p "$pid" -o command=)"
  case "$command" in
    *"$BINARY"*"$CONFIG_FILE"* | *"$LEGACY_BINARY"*"$CONFIG_FILE"*) echo "$pid" ;;
    *) die "PID $pid is not this staging backend" ;;
  esac
}

stop_app() {
  set_paths
  if ! pid_is_running; then
    if [[ "$(uname -s)" == Darwin ]] && launchctl print "gui/$(id -u)/$LAUNCH_LABEL" >/dev/null 2>&1; then launchctl remove "$LAUNCH_LABEL"; fi
    rm -f "$PID_FILE"; echo "Staging application is already stopped."; return
  fi
  local pid attempt
  pid="$(verified_pid)"
  echo "Stopping staging application (PID $pid)..."
  if [[ "$(uname -s)" == Darwin ]] && launchctl print "gui/$(id -u)/$LAUNCH_LABEL" >/dev/null 2>&1; then
    launchctl remove "$LAUNCH_LABEL"
  else
    kill -TERM "$pid"
  fi
  for attempt in {1..20}; do
    if ! kill -0 "$pid" 2>/dev/null; then rm -f "$PID_FILE"; echo "Stopped."; return; fi
    sleep 0.25
  done
  die "PID $pid did not stop after 5 seconds"
}

all_components_installed() {
  local component
  for component in "${COMPONENTS[@]}"; do [[ -f "$(component_dir "$component")/component.manifest" ]] || return 1; done
}

create_or_migrate_config() {
  local example="$BACKEND_DIR/server.config.example.toml" port temporary
  if [[ ! -f "$CONFIG_FILE" ]]; then
    [[ -f "$example" ]] || die "backend artifact lacks server.config.example.toml"
    case "$DEFAULT_LISTEN_ADDR" in 127.0.0.1:* | localhost:* | '[::1]:'*) ;; *) die "staging address must be loopback-only" ;; esac
    cp "$example" "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
    echo "Created persistent configuration: $CONFIG_FILE"
  fi
  port="$(read_listen_addr 2>/dev/null | awk -F: '{print $NF}')"
  [[ -n "$port" ]] || port="${DEFAULT_LISTEN_ADDR##*:}"
  temporary="$CONFIG_FILE.tmp"
  awk -v initial="$DEFAULT_LISTEN_ADDR" -v books="$BOOKS_DIR" -v launcher="$LAUNCHER_DIR/dist" -v runtime="$RUNTIME_DIR" -v audit="$LOG_DIR/ops-audit.jsonl" -v redirect="http://localhost:${port}/api/auth/google/callback" '
    /^listen_addr = / && !seen { print "listen_addr = \"" initial "\""; seen=1; next }
    /^books_dir = / { print "books_dir = \"" books "\""; next }
    /^frontend_dist = / { print "frontend_dist = \"" launcher "\""; next }
    /^dev_artifacts_dir = / { print "dev_artifacts_dir = \"" runtime "\""; next }
    /^ops_audit_log = / { print "ops_audit_log = \"" audit "\""; next }
    /^redirect_url = / && $0 ~ /localhost/ { print "redirect_url = \"" redirect "\""; next }
    { print }
  ' "$CONFIG_FILE" >"$temporary"
  mv "$temporary" "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
}

start_app() {
  local attempt url launch_pid
  ensure_layout
  all_components_installed || die "all four components must be installed before start"
  check_compatibility
  [[ -x "$BINARY" ]] || die "backend executable is missing"
  [[ -f "$BACKEND_DIR/component.manifest" ]] && create_or_migrate_config
  if pid_is_running; then echo "Staging application is already running (PID $(cat "$PID_FILE"))."; return; fi
  rm -f "$PID_FILE"
  echo "Starting staging application..."
  if [[ "$(uname -s)" == Darwin ]]; then
    launchctl remove "$LAUNCH_LABEL" >/dev/null 2>&1 || true
    launchctl submit -l "$LAUNCH_LABEL" -o "$APP_LOG" -e "$APP_LOG" -- "$BINARY" "$CONFIG_FILE"
    for attempt in {1..20}; do
      launch_pid="$(launchctl print "gui/$(id -u)/$LAUNCH_LABEL" 2>/dev/null | awk '/pid =/ { print $3; exit }')"
      if [[ "$launch_pid" =~ ^[0-9]+$ ]]; then echo "$launch_pid" >"$PID_FILE"; break; fi
      sleep 0.1
    done
    [[ -f "$PID_FILE" ]] || die "launchd did not report a backend PID"
  else
    (
      cd "$BACKEND_DIR"
      export LD_LIBRARY_PATH="$ENGINE_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
      nohup "$BINARY" "$CONFIG_FILE" </dev/null >>"$APP_LOG" 2>&1 &
      echo "$!" >"$PID_FILE"
    )
  fi
  url="$(health_url)"
  for attempt in {1..30}; do
    if curl -fsS "$url" >/dev/null 2>&1; then echo "Healthy: $url"; echo "Open: ${url%/api/health}"; return; fi
    if ! pid_is_running; then
      [[ "$(uname -s)" != Darwin ]] || launchctl remove "$LAUNCH_LABEL" >/dev/null 2>&1 || true
      rm -f "$PID_FILE"
      tail -n 30 "$APP_LOG" >&2 || true
      die "backend exited during startup"
    fi
    sleep 0.5
  done
  tail -n 30 "$APP_LOG" >&2 || true
  [[ "$(uname -s)" != Darwin ]] || launchctl remove "$LAUNCH_LABEL" >/dev/null 2>&1 || true
  rm -f "$PID_FILE"
  die "health check timed out"
}

validate_archive() {
  local artifact="$1" entry
  [[ -f "$artifact" ]] || die "artifact does not exist: $artifact"
  tar -tzf "$artifact" >/dev/null || die "artifact is not a readable tar.gz"
  while IFS= read -r entry; do [[ "$entry" != /* && "/$entry/" != *"/../"* ]] || die "unsafe archive path: $entry"; done < <(tar -tzf "$artifact")
}

payload_hash() {
  local component="$1" root="$2"
  case "$component" in
    engine) find "$root/lib" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}' ;;
    backend) shasum -a 256 "$root/ledgerzero-backend" | awk '{print $1}' ;;
    launcher) find "$root/dist" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}' ;;
    runtime-frontends) find "$root/workflows" -type f -exec shasum -a 256 {} \; | awk '{print $1}' | sort | shasum -a 256 | awk '{print $1}' ;;
  esac
}

install_component() {
  local expected="$1" artifact="$2" extract top manifest actual expected_hash actual_hash target saved name artifact_hash deployed_at description key
  validate_archive "$artifact"
  extract="$(mktemp -d "$STAGE_ROOT/.extract.XXXXXX")"
  tar -xzf "$artifact" -C "$extract"
  [[ "$(find "$extract" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 1 ]] || die "artifact must have one top-level directory"
  top="$(find "$extract" -mindepth 1 -maxdepth 1 -type d -print -quit)"
  manifest="$top/component.manifest"
  [[ -f "$manifest" ]] || die "artifact lacks component.manifest"
  for key in manifest_format component component_version payload_sha256 source_revision platform packaged_at; do require_manifest_key "$manifest" "$key"; done
  [[ "$(manifest_value "$manifest" manifest_format)" == 1 ]] || die "unsupported manifest format"
  actual="$(manifest_value "$manifest" component)"
  [[ "$actual" == "$expected" ]] || die "artifact is $actual, not $expected"
  [[ "$(manifest_value "$manifest" platform)" == "$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m)" ]] || die "artifact platform does not match this host"
  case "$expected" in
    engine)
      require_manifest_key "$manifest" engine_api; require_manifest_key "$manifest" storage_format; require_manifest_key "$manifest" rust_abi
      [[ -f "$top/lib/libledgerzero_engine.dylib" || -f "$top/lib/libledgerzero_engine.so" ]] || die "engine library is missing"
      ;;
    backend)
      require_manifest_key "$manifest" backend_api; require_manifest_key "$manifest" requires_engine_api; require_manifest_key "$manifest" requires_storage_format; require_manifest_key "$manifest" requires_rust_abi
      [[ -x "$top/ledgerzero-backend" ]] || die "backend executable is missing"
      description="$(file "$top/ledgerzero-backend")"
      case "$(uname -s)" in Darwin) [[ "$description" == *Mach-O* && "$description" == *"$(uname -m)"* ]] || die "backend is not a native macOS executable" ;; Linux) [[ "$description" == *ELF* ]] || die "backend is not a Linux executable" ;; esac
      ;;
    launcher) require_manifest_key "$manifest" requires_backend_api; [[ -f "$top/dist/index.html" ]] || die "launcher index is missing" ;;
    runtime-frontends) require_manifest_key "$manifest" requires_backend_api; require_manifest_key "$manifest" runtime_frontend_api; [[ -d "$top/workflows" ]] || die "workflow directory is missing" ;;
  esac
  expected_hash="$(manifest_value "$manifest" payload_sha256)"
  actual_hash="$(payload_hash "$expected" "$top")"
  [[ "$actual_hash" == "$expected_hash" ]] || die "$expected payload hash mismatch"
  check_compatibility "$expected" "$manifest"

  mkdir -p "$ARTIFACTS_DIR/$expected"
  artifact_hash="$(shasum -a 256 "$artifact" | awk '{print $1}')"
  name="$(basename "$artifact")"
  saved="$ARTIFACTS_DIR/$expected/$name"
  if [[ -f "$saved" ]]; then
    [[ "$(shasum -a 256 "$saved" | awk '{print $1}')" == "$artifact_hash" ]] || die "preserved artifact name collision"
  else
    cp "$artifact" "$saved.tmp"; mv "$saved.tmp" "$saved"
  fi
  target="$(component_dir "$expected")"
  [[ "$target" == "$COMPONENTS_DIR/$expected" ]] || die "component target safety check failed"
  rm -rf "$target"
  mv "$top" "$target"
  rmdir "$extract"
  [[ "$expected" != backend ]] || chmod +x "$BINARY"
  deployed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  { echo "artifact_path=$saved"; echo "artifact_sha256=$artifact_hash"; echo "deployed_at=$deployed_at"; } >"$RUN_DIR/$expected.env.pending"
  echo "$saved" >"$RUN_DIR/$expected.current.pending"
  echo "Installed $expected: $saved"
}

commit_component() {
  local component="$1"
  [[ -f "$RUN_DIR/$component.env.pending" && -f "$RUN_DIR/$component.current.pending" ]] || die "missing pending deployment record for $component"
  mv "$RUN_DIR/$component.env.pending" "$RUN_DIR/$component.env"
  mv "$RUN_DIR/$component.current.pending" "$RUN_DIR/$component.current"
}

deploy_component() {
  local component="$1" artifact="$2" was_running=false
  ensure_layout
  pid_is_running && was_running=true
  if [[ "$component" == engine || "$component" == backend ]]; then stop_app; fi
  install_component "$component" "$artifact"
  if [[ "$SCRIPT_PATH" != "$DEPLOY_DIR/fpa-stage" ]]; then cp "$SCRIPT_PATH" "$DEPLOY_DIR/fpa-stage"; chmod +x "$DEPLOY_DIR/fpa-stage"; fi
  [[ -f "$BACKEND_DIR/component.manifest" ]] && create_or_migrate_config
  if [[ "$component" == engine || "$component" == backend ]]; then
    if [[ "$was_running" == true ]] || all_components_installed; then start_app; fi
  elif pid_is_running; then
    curl -fsS "$(health_url)" >/dev/null || die "health failed after $component replacement"
    echo "Backend remains healthy; no restart required."
  elif all_components_installed; then start_app
  fi
  commit_component "$component"
}

recorded_artifact() {
  local file="$RUN_DIR/$1.current" artifact
  [[ -f "$file" ]] || die "no recorded artifact for $1"
  artifact="$(cat "$file")"
  [[ -f "$artifact" ]] || die "recorded artifact is missing: $artifact"
  echo "$artifact"
}

build_deploy() {
  [[ -n "$SOURCE_ROOT" ]] || die "build-deploy must use the source-repository script"
  local output component artifact
  output="$(mktemp /tmp/fpa-components.XXXXXX)"
  "$SOURCE_ROOT/scripts/package.sh" --locked | tee "$output"
  ensure_layout
  stop_app
  for component in "${COMPONENTS[@]}"; do
    artifact="$(sed -n "s/^Artifact\[$component\]: //p" "$output" | tail -1)"
    [[ -n "$artifact" ]] || die "package output lacks $component"
    install_component "$component" "$SOURCE_ROOT/$artifact"
  done
  rm -f "$output"
  cp "$SCRIPT_PATH" "$DEPLOY_DIR/fpa-stage"; chmod +x "$DEPLOY_DIR/fpa-stage"
  create_or_migrate_config
  start_app
  for component in "${COMPONENTS[@]}"; do commit_component "$component"; done
  if [[ -d "$STAGE_ROOT/app" ]]; then
    [[ ! -L "$STAGE_ROOT/app" ]] || die "refusing to remove a symlinked legacy app directory"
    rm -rf "$STAGE_ROOT/app"
    echo "Removed the superseded L2 monolithic app directory."
  fi
}

recover() {
  ensure_layout
  local component artifacts=()
  for component in "${COMPONENTS[@]}"; do artifacts+=("$(recorded_artifact "$component")"); done
  stop_app
  [[ "$COMPONENTS_DIR" == "$STAGE_ROOT/components" ]] || die "component set safety check failed"
  rm -rf "$COMPONENTS_DIR"
  mkdir -p "$COMPONENTS_DIR"
  local index=0
  for component in "${COMPONENTS[@]}"; do install_component "$component" "${artifacts[$index]}"; index=$((index + 1)); done
  start_app
  for component in "${COMPONENTS[@]}"; do commit_component "$component"; done
}

show_status() {
  ensure_layout
  echo "Staging root:  $STAGE_ROOT"
  echo "Configuration: $CONFIG_FILE"
  echo "Books:         $BOOKS_DIR"
  echo "Components:"
  local component manifest record
  for component in "${COMPONENTS[@]}"; do
    manifest="$(component_dir "$component")/component.manifest"; record="$RUN_DIR/$component.env"
    if [[ -f "$manifest" ]]; then
      printf '  %-18s version=%s payload=%s\n' "$component" "$(manifest_value "$manifest" component_version)" "$(manifest_value "$manifest" payload_sha256)"
      [[ -f "$record" ]] && sed 's/^/    /' "$record"
    else printf '  %-18s not installed\n' "$component"; fi
  done
  check_compatibility
  if pid_is_running; then echo "Process: running (PID $(cat "$PID_FILE"))"; curl -fsS "$(health_url)"; echo; else echo "Process: stopped"; return 1; fi
}

validate_stage_root
set_paths
command="${1:-}"
case "$command" in
  build-deploy) [[ $# -eq 1 ]] || die "build-deploy accepts no arguments"; build_deploy ;;
  deploy-component) [[ $# -eq 3 ]] || die "usage: $0 deploy-component <name> <artifact>"; deploy_component "$2" "$3" ;;
  redeploy-component) [[ $# -eq 2 ]] || die "usage: $0 redeploy-component <name>"; ensure_layout; deploy_component "$2" "$(recorded_artifact "$2")" ;;
  recover) [[ $# -eq 1 ]] || die "recover accepts no arguments"; recover ;;
  start) [[ $# -eq 1 ]] || die "start accepts no arguments"; start_app ;;
  stop) [[ $# -eq 1 ]] || die "stop accepts no arguments"; stop_app ;;
  restart) [[ $# -eq 1 ]] || die "restart accepts no arguments"; stop_app; start_app ;;
  status) [[ $# -eq 1 ]] || die "status accepts no arguments"; show_status ;;
  logs) [[ $# -eq 1 ]] || die "logs accepts no arguments"; tail -f "$APP_LOG" ;;
  config) [[ $# -eq 1 ]] || die "config accepts no arguments"; echo "$CONFIG_FILE" ;;
  help | -h | --help) usage ;;
  "") usage; exit 1 ;;
  *) usage >&2; die "unknown command: $command" ;;
esac
