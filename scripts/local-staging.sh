#!/usr/bin/env bash
# Build, deploy, and operate an isolated local FPA staging installation on macOS.
set -euo pipefail

SCRIPT_PATH="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")"
SCRIPT_DIR="$(dirname "$SCRIPT_PATH")"
SOURCE_ROOT_CANDIDATE="$(cd "$SCRIPT_DIR/.." 2>/dev/null && pwd -P || true)"
if [[ -f "$SOURCE_ROOT_CANDIDATE/Cargo.toml" && -x "$SOURCE_ROOT_CANDIDATE/scripts/package.sh" ]]; then
  SOURCE_ROOT="$SOURCE_ROOT_CANDIDATE"
else
  SOURCE_ROOT=""
fi

STAGE_ROOT="${FPA_STAGE_ROOT:-${HOME}/Deployments/FPA-Staging}"
DEFAULT_LISTEN_ADDR="${FPA_STAGE_ADDR:-127.0.0.1:8081}"

die() {
  echo "Error: $*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: local-staging.sh <command> [artifact]

Commands:
  build-deploy         Build a new artifact from this source checkout, deploy it, and start it.
  deploy <artifact>    Preserve a copy of the artifact, replace app/, and start it.
  redeploy [artifact]  Redeploy the given artifact, or the currently recorded artifact.
  start                Start the installed staging application.
  stop                 Stop the installed staging application.
  restart              Stop and start the installed staging application.
  status               Show the process, health, installed artifact, and important paths.
  logs                 Follow the application log.
  config               Print the persistent configuration path.

Environment overrides:
  FPA_STAGE_ROOT        Staging root (default: $HOME/Deployments/FPA-Staging)
  FPA_STAGE_ADDR        Initial loopback listen address (default: 127.0.0.1:8081)

The script never replaces config/, data/, logs/, artifacts/, or deploy/ during
application deployment. It deletes and recreates only app/.
EOF
}

validate_stage_root() {
  case "$STAGE_ROOT" in
    "" | "/" | "$HOME")
      die "unsafe FPA_STAGE_ROOT: $STAGE_ROOT"
      ;;
  esac
  if [[ -n "$SOURCE_ROOT" && "$STAGE_ROOT" == "$SOURCE_ROOT" ]]; then
    die "FPA_STAGE_ROOT must not be the source repository: $STAGE_ROOT"
  fi
  if [[ -L "$STAGE_ROOT" ]]; then
    die "FPA_STAGE_ROOT must not be a symbolic link: $STAGE_ROOT"
  fi
}

set_paths() {
  APP_DIR="$STAGE_ROOT/app"
  CONFIG_DIR="$STAGE_ROOT/config"
  CONFIG_FILE="$CONFIG_DIR/server.config.toml"
  DATA_DIR="$STAGE_ROOT/data"
  BOOKS_DIR="$DATA_DIR/books"
  RUNTIME_FRONTENDS_DIR="$DATA_DIR/runtime-frontends"
  BACKUPS_DIR="$DATA_DIR/backups"
  LOG_DIR="$STAGE_ROOT/logs"
  APP_LOG="$LOG_DIR/application.log"
  ARTIFACTS_DIR="$STAGE_ROOT/artifacts"
  DEPLOY_DIR="$STAGE_ROOT/deploy"
  RUN_DIR="$STAGE_ROOT/run"
  PID_FILE="$RUN_DIR/fpa.pid"
  INSTALL_RECORD="$RUN_DIR/installed-artifact.env"
  CURRENT_ARTIFACT_FILE="$RUN_DIR/current-artifact"
  BINARY="$APP_DIR/ledgerzero-backend"
}

reject_managed_symlinks() {
  local path
  for path in "$APP_DIR" "$CONFIG_DIR" "$DATA_DIR" "$LOG_DIR" "$ARTIFACTS_DIR" "$DEPLOY_DIR" "$RUN_DIR"; do
    if [[ -L "$path" ]]; then
      die "managed staging path must not be a symbolic link: $path"
    fi
  done
}

ensure_layout() {
  validate_stage_root
  mkdir -p "$STAGE_ROOT"
  STAGE_ROOT="$(cd "$STAGE_ROOT" && pwd -P)"
  set_paths
  reject_managed_symlinks
  mkdir -p \
    "$CONFIG_DIR" \
    "$BOOKS_DIR" \
    "$RUNTIME_FRONTENDS_DIR" \
    "$BACKUPS_DIR" \
    "$LOG_DIR" \
    "$ARTIFACTS_DIR" \
    "$DEPLOY_DIR" \
    "$RUN_DIR"
}

read_listen_addr() {
  awk -F '"' '/^[[:space:]]*listen_addr[[:space:]]*=/ { print $2; exit }' "$CONFIG_FILE"
}

health_url() {
  local address
  address="$(read_listen_addr)"
  [[ -n "$address" ]] || die "listen_addr is missing from $CONFIG_FILE"
  printf 'http://%s/api/health\n' "$address"
}

pid_is_running() {
  [[ -f "$PID_FILE" ]] || return 1
  local pid
  pid="$(cat "$PID_FILE")"
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  kill -0 "$pid" 2>/dev/null
}

verified_pid() {
  [[ -f "$PID_FILE" ]] || return 1
  local pid command
  pid="$(cat "$PID_FILE")"
  [[ "$pid" =~ ^[0-9]+$ ]] || die "invalid PID file: $PID_FILE"
  kill -0 "$pid" 2>/dev/null || return 1
  command="$(ps -p "$pid" -o command=)"
  case "$command" in
    *"$BINARY"*"$CONFIG_FILE"*) printf '%s\n' "$pid" ;;
    *) die "PID $pid does not belong to this staging installation; refusing to stop it" ;;
  esac
}

stop_app() {
  set_paths
  if ! pid_is_running; then
    rm -f "$PID_FILE"
    echo "Staging application is already stopped."
    return
  fi

  local pid attempt
  pid="$(verified_pid)"
  echo "Stopping staging application (PID $pid)..."
  kill -TERM "$pid"
  for attempt in {1..20}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      rm -f "$PID_FILE"
      echo "Stopped."
      return
    fi
    sleep 0.25
  done
  die "PID $pid did not stop after 5 seconds; inspect it manually"
}

start_app() {
  ensure_layout
  [[ -x "$BINARY" ]] || die "no deployed executable at $BINARY; run deploy first"
  [[ -f "$CONFIG_FILE" ]] || die "configuration is missing: $CONFIG_FILE"

  if pid_is_running; then
    echo "Staging application is already running (PID $(cat "$PID_FILE"))."
    return
  fi
  rm -f "$PID_FILE"

  echo "Starting staging application..."
  (
    cd "$APP_DIR"
    nohup "$BINARY" "$CONFIG_FILE" >>"$APP_LOG" 2>&1 &
    printf '%s\n' "$!" >"$PID_FILE"
  )

  local attempt url
  url="$(health_url)"
  for attempt in {1..30}; do
    if curl -fsS "$url" >/dev/null 2>&1; then
      echo "Healthy: $url"
      echo "Open:    ${url%/api/health}"
      return
    fi
    if ! pid_is_running; then
      echo "Application exited during startup. Recent log output:" >&2
      tail -n 30 "$APP_LOG" >&2 || true
      die "staging application failed to start"
    fi
    sleep 0.5
  done

  echo "Health check timed out. Recent log output:" >&2
  tail -n 30 "$APP_LOG" >&2 || true
  die "staging application did not become healthy at $url"
}

validate_artifact_archive() {
  local artifact="$1" entry file_description
  [[ -f "$artifact" ]] || die "artifact does not exist: $artifact"
  tar -tzf "$artifact" >/dev/null || die "artifact is not a readable .tar.gz archive: $artifact"
  while IFS= read -r entry; do
    [[ "$entry" != /* ]] || die "artifact contains an absolute path: $entry"
    case "/$entry/" in
      *"/../"*) die "artifact contains a parent-directory path: $entry" ;;
    esac
  done < <(tar -tzf "$artifact")

  file_description="$(file "$artifact")"
  [[ "$file_description" == *"gzip compressed data"* ]] || die "artifact is not gzip-compressed: $artifact"
}

preserve_artifact() {
  local source="$1" name destination source_hash destination_hash source_absolute
  source_absolute="$(cd "$(dirname "$source")" && pwd -P)/$(basename "$source")"
  name="$(basename "$source")"
  destination="$ARTIFACTS_DIR/$name"
  source_hash="$(shasum -a 256 "$source_absolute" | awk '{print $1}')"

  if [[ -f "$destination" ]]; then
    destination_hash="$(shasum -a 256 "$destination" | awk '{print $1}')"
    [[ "$source_hash" == "$destination_hash" ]] || die "artifact name already exists with different content: $destination"
  elif [[ "$source_absolute" != "$destination" ]]; then
    cp "$source_absolute" "$destination.tmp"
    mv "$destination.tmp" "$destination"
  fi

  PRESERVED_ARTIFACT="$destination"
  ARTIFACT_HASH="$source_hash"
}

create_config_once() {
  local example="$1" port
  [[ -f "$CONFIG_FILE" ]] && return
  [[ -f "$example" ]] || die "artifact does not contain server.config.example.toml"

  case "$DEFAULT_LISTEN_ADDR" in
    127.0.0.1:* | localhost:* | '[::1]:'*) ;;
    *) die "FPA_STAGE_ADDR must be loopback-only, not $DEFAULT_LISTEN_ADDR" ;;
  esac
  port="${DEFAULT_LISTEN_ADDR##*:}"

  awk \
    -v listen_addr="$DEFAULT_LISTEN_ADDR" \
    -v books_dir="$BOOKS_DIR" \
    -v frontend_dist="$APP_DIR/frontend/dist" \
    -v runtime_frontends_dir="$RUNTIME_FRONTENDS_DIR" \
    -v ops_audit_log="$LOG_DIR/ops-audit.jsonl" \
    -v redirect_url="http://localhost:${port}/api/auth/google/callback" '
      /^listen_addr = / { print "listen_addr = \"" listen_addr "\""; next }
      /^books_dir = / { print "books_dir = \"" books_dir "\""; next }
      /^frontend_dist = / { print "frontend_dist = \"" frontend_dist "\""; next }
      /^dev_artifacts_dir = / { print "dev_artifacts_dir = \"" runtime_frontends_dir "\""; next }
      /^ops_audit_log = / { print "ops_audit_log = \"" ops_audit_log "\""; next }
      /^redirect_url = / { print "redirect_url = \"" redirect_url "\""; next }
      { print }
    ' "$example" >"$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
  echo "Created persistent configuration: $CONFIG_FILE"
  echo "Google OAuth credentials are blank; edit this file before testing Google login."
}

deploy_artifact() {
  local artifact="$1" extract_dir top_count top_dir binary_description deployed_at
  ensure_layout
  validate_artifact_archive "$artifact"
  preserve_artifact "$artifact"

  extract_dir="$(mktemp -d "$STAGE_ROOT/.extract.XXXXXX")"
  tar -xzf "$PRESERVED_ARTIFACT" -C "$extract_dir"

  top_count="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  [[ "$top_count" == "1" ]] || die "artifact must contain exactly one top-level directory"
  top_dir="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d -print -quit)"
  [[ -x "$top_dir/ledgerzero-backend" ]] || die "artifact is missing ledgerzero-backend"
  [[ -f "$top_dir/frontend/dist/index.html" ]] || die "artifact is missing frontend/dist/index.html"

  binary_description="$(file "$top_dir/ledgerzero-backend")"
  [[ "$binary_description" == *"Mach-O"* ]] || die "local Mac staging requires a macOS executable: $binary_description"
  if [[ "$(uname -m)" == "arm64" && "$binary_description" != *"arm64"* ]]; then
    die "this ARM64 Mac requires an ARM64 artifact: $binary_description"
  fi

  stop_app
  [[ "$APP_DIR" == "$STAGE_ROOT/app" ]] || die "internal safety check failed for app directory"
  rm -rf "$APP_DIR"
  mv "$top_dir" "$APP_DIR"
  chmod +x "$BINARY"

  create_config_once "$APP_DIR/server.config.example.toml"
  if [[ "$SCRIPT_PATH" != "$DEPLOY_DIR/fpa-stage" ]]; then
    cp "$SCRIPT_PATH" "$DEPLOY_DIR/fpa-stage"
  fi
  chmod +x "$DEPLOY_DIR/fpa-stage"

  deployed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  {
    printf 'artifact_path=%s\n' "$PRESERVED_ARTIFACT"
    printf 'artifact_sha256=%s\n' "$ARTIFACT_HASH"
    printf 'deployed_at=%s\n' "$deployed_at"
  } >"$INSTALL_RECORD"
  printf '%s\n' "$PRESERVED_ARTIFACT" >"$CURRENT_ARTIFACT_FILE"
  rmdir "$extract_dir"

  echo "Installed: $PRESERVED_ARTIFACT"
  echo "SHA-256:  $ARTIFACT_HASH"
  start_app
}

recorded_artifact() {
  [[ -f "$CURRENT_ARTIFACT_FILE" ]] || die "no installed artifact record; provide an artifact"
  local artifact_path
  artifact_path="$(cat "$CURRENT_ARTIFACT_FILE")"
  [[ -n "$artifact_path" && -f "$artifact_path" ]] || die "recorded artifact is unavailable: $artifact_path"
  printf '%s\n' "$artifact_path"
}

build_deploy() {
  [[ -n "$SOURCE_ROOT" ]] || die "build-deploy must be run from the source-repository script"
  local output_file artifact_relative artifact
  output_file="$(mktemp /tmp/fpa-package-output.XXXXXX)"
  "$SOURCE_ROOT/scripts/package.sh" --locked | tee "$output_file"
  artifact_relative="$(awk -F 'Artifact: ' '/^Artifact: / { print $2 }' "$output_file" | tail -n 1)"
  rm -f "$output_file"
  [[ -n "$artifact_relative" ]] || die "package script did not report an artifact"
  artifact="$SOURCE_ROOT/$artifact_relative"
  deploy_artifact "$artifact"
}

show_status() {
  set_paths
  echo "Staging root: $STAGE_ROOT"
  echo "Application:  $APP_DIR"
  echo "Configuration: $CONFIG_FILE"
  echo "Books:         $BOOKS_DIR"
  if [[ -f "$INSTALL_RECORD" ]]; then
    echo "Installed artifact:"
    sed 's/^/  /' "$INSTALL_RECORD"
  else
    echo "Installed artifact: none"
  fi

  if pid_is_running; then
    echo "Process: running (PID $(cat "$PID_FILE"))"
    local url
    url="$(health_url)"
    if curl -fsS "$url"; then
      echo
      echo "Health: healthy ($url)"
    else
      echo "Health: unavailable ($url)"
      return 1
    fi
  else
    echo "Process: stopped"
    return 1
  fi
}

validate_stage_root
set_paths

command="${1:-}"
case "$command" in
  build-deploy)
    [[ $# -eq 1 ]] || die "build-deploy does not accept an artifact argument"
    build_deploy
    ;;
  deploy)
    [[ $# -eq 2 ]] || die "usage: $0 deploy <artifact>"
    deploy_artifact "$2"
    ;;
  redeploy)
    [[ $# -le 2 ]] || die "usage: $0 redeploy [artifact]"
    ensure_layout
    deploy_artifact "${2:-$(recorded_artifact)}"
    ;;
  start)
    [[ $# -eq 1 ]] || die "start does not accept arguments"
    start_app
    ;;
  stop)
    [[ $# -eq 1 ]] || die "stop does not accept arguments"
    stop_app
    ;;
  restart)
    [[ $# -eq 1 ]] || die "restart does not accept arguments"
    stop_app
    start_app
    ;;
  status)
    [[ $# -eq 1 ]] || die "status does not accept arguments"
    show_status
    ;;
  logs)
    [[ $# -eq 1 ]] || die "logs does not accept arguments"
    [[ -f "$APP_LOG" ]] || die "application log does not exist: $APP_LOG"
    tail -f "$APP_LOG"
    ;;
  config)
    [[ $# -eq 1 ]] || die "config does not accept arguments"
    echo "$CONFIG_FILE"
    ;;
  -h | --help | help)
    usage
    ;;
  "")
    usage
    exit 1
    ;;
  *)
    usage >&2
    die "unknown command: $command"
    ;;
esac
