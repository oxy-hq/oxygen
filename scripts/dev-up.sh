#!/usr/bin/env bash
# `just up` / `down` / `status` / `stacks` — local stacks, as many per machine as you have checkouts.
#
#   One checkout = one stack: its own backend (`oxy serve --enterprise`) and Vite, on the ports
#   of the SLOT it holds (scripts/dev-slots.py: API 3000+10N, internal 3001+10N, Vite 5173+10N;
#   slot 0 = 3000/3001/5173). A checkout keeps its slot, so its URLs stay put across runs.
#   Every stack shares ONE data layer: the oxy-postgres container (:15432, db `oxy`), the local
#   Airhouse (:8080 admin, :5445 wire) and, with OXY_OBSERVABILITY_BACKEND=clickhouse,
#   oxy-clickhouse. `oxy start --db-only` brings it up when it is not healthy; nothing else
#   restarts it, so one stack's restart never drops another's database.
#
#   up       build, shared data layer, this checkout's backend, `oxy dev seed --fixtures`, Vite.
#            Reuses whatever is ours and healthy; restarts the backend only when the binary
#            changed. Writes .oxy-dev/state.json, the file an agent reads next.
#   down     Stops this checkout's backend + Vite.
#   status   This checkout's stack, live. Exit 1 when it is not serving.
#   stacks   Every stack on this machine, and the shared data layer.
#
#   up flags    --no-build       use the existing binary ($OXY_BIN, else target/debug/oxy)
#               --no-seed        skip `oxy dev seed`
#               --no-frontend    API only, no Vite
#               --frontend-only  Vite only, proxying to a backend already running on this machine
#                                (this checkout's, else the lowest slot's; OXY_DEV_BACKEND_URL
#                                names one). No cargo, no Docker: for web-app-only changes
#               --restart        restart backend + Vite even when healthy
#               --clean          wipe the shared Postgres/ClickHouse/Airhouse volumes first
#               --no-airhouse    this backend gets no Airhouse (and none is started for it)
#   down flags  --db             also stop the shared data layer's containers
#               --release        give this checkout's slot back
#   --force     let --clean / --db go ahead while other checkouts' stacks are running
#
# Env: OXY_DEV_SLOT=N pins the slot. OXY_DEV_HOME moves the machine-wide registry.
# Cloud/enterprise mode only — never `--local`. Sign in at <frontend_url>/dev-login?as=<persona>.
# Guide: .claude/skills/oxy-run-and-verify/SKILL.md
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd -P)"
DEV_DIR="$REPO/.oxy-dev"
RUN_DIR="$DEV_DIR/run"
LOG_DIR="$DEV_DIR/logs"
STATE_JSON="$DEV_DIR/state.json"
STATE_WRITER="$REPO/scripts/dev-up-state.py"
SLOTS="$REPO/scripts/dev-slots.py"
SELF="$REPO/scripts/dev-up.sh"
export OXY_DEV_REPO="$REPO" # names this checkout in another run's "waiting for … lock" line

PG_PORT=15432
DATABASE_URL="postgresql://postgres:postgres@localhost:$PG_PORT/oxy"
AIRHOUSE_URL="http://localhost:8080"
CLICKHOUSE_URL="http://localhost:8123"
DATA_CONTAINERS="oxy-postgres oxy-clickhouse oxy-airhouse-dp oxy-airhouse-cp oxy-airhouse-minio"
BIN="${OXY_BIN:-$REPO/target/debug/oxy}"
# Recorded pids are verified against these before anything is signalled; dev-slots.py MATCH
# carries the same strings. The legacy one is the `oxy start` an older `just up` ran.
BACKEND_MATCH="serve --enterprise"
LEGACY_BACKEND_MATCH="start --enterprise"
FRONTEND_MATCH="--strictPort"

if [ -t 1 ]; then B=$'\033[1m' Y=$'\033[33m' R=$'\033[31m' Z=$'\033[0m'; else B='' Y='' R='' Z=''; fi
step() { printf '\n%s==> %s%s\n' "$B" "$*" "$Z"; }
note() { printf '    %s\n' "$*"; }
warn() { printf '%swarn:%s %s\n' "$Y" "$Z" "$*" >&2; }
die() {
  printf '%serror:%s %s\n' "$R" "$Z" "$*" >&2
  exit 1
}
rel() { printf '%s' "${1#"$REPO"/}"; }
slots() { python3 "$SLOTS" "$@"; }

rget() { if [ -f "$RUN_DIR/$1" ]; then cat "$RUN_DIR/$1"; fi; }
rset() { printf '%s\n' "$2" >"$RUN_DIR/$1"; }

# A value from the environment, else from the repo .env (dotenv never overrides the env).
env_or_dotenv() {
  local v="${!1:-}"
  if [ -z "$v" ] && [ -f "$REPO/.env" ]; then
    v=$(grep -E "^$1=" "$REPO/.env" | head -1 | cut -d= -f2- |
      sed -E -e 's/[[:space:]]+#.*$//' -e "s/^[\"']//" -e "s/[\"']$//" || true)
  fi
  printf '%s' "$v"
}

alive() { [ -n "${1:-}" ] && kill -0 "$1" 2>/dev/null; }
group_alive() { [ -n "${1:-}" ] && kill -0 -- "-$1" 2>/dev/null; }
http_ok() { curl -sf --noproxy '*' -o /dev/null --max-time 3 "$1" 2>/dev/null; }
tcp_ok() { python3 -c 'import socket,sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), 2)' "$1" 2>/dev/null; }

# The recorded pid's process group, if it is still ours. spawn() makes every child a session
# leader, so a live leader must lead its own group and run one of the MATCHes (a pid recycled
# after a reboot does neither); a group whose leader died but members linger is ours too.
our_group() { # run-file-name match...
  local pid pgid cmd m name="$1"
  shift
  pid=$(rget "$name.pid")
  [ -n "$pid" ] || return 0
  if alive "$pid"; then
    pgid=$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
    cmd=$(ps -o command= -p "$pid" 2>/dev/null || true)
    [ "$pgid" = "$pid" ] || return 0
    for m in "$@"; do
      case "$cmd" in *"$m"*) printf '%s' "$pid" && return 0 ;; esac
    done
  elif group_alive "$pid"; then
    printf '%s' "$pid"
  fi
  return 0
}
backend_group() { our_group backend "$BACKEND_MATCH" "$LEGACY_BACKEND_MATCH"; }
frontend_group() { our_group frontend "$FRONTEND_MATCH"; }

fingerprint() {
  if [ ! -e "$1" ]; then echo missing; elif [ "$(uname -s)" = Darwin ]; then
    stat -f '%m:%z' "$1"
  else stat -c '%Y:%s' "$1"; fi
}

fresh_log() { # name -> a log path that did not exist before (never reuse or append)
  local base path n=1
  base="$LOG_DIR/$1-$(date +%Y%m%d-%H%M%S)"
  path="$base.log"
  while [ -e "$path" ]; do
    n=$((n + 1))
    path="$base-$n.log"
  done
  : >"$path"
  printf '%s' "$path"
}

# Detached in a new session: survives the calling shell (and an agent's tool call ending),
# and `down` can signal the whole tree (pnpm -> node) as one process group.
spawn() { # log argv... -> pid
  python3 - "$REPO" "$@" <<'PY'
import subprocess, sys
cwd, log, argv = sys.argv[1], sys.argv[2], sys.argv[3:]
with open(log, "ab") as out:
    p = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=out,
                         stderr=subprocess.STDOUT, start_new_session=True)
print(p.pid)
PY
}

stop() { # run-file-name grace-seconds group
  local i=0
  if [ -n "$3" ]; then
    note "stopping $1 (process group $3)"
    kill -TERM -- "-$3" 2>/dev/null || true
    while group_alive "$3" && [ "$i" -lt "$2" ]; do
      sleep 1
      i=$((i + 1))
    done
    if group_alive "$3"; then
      warn "$1 still running ${2}s after SIGTERM — SIGKILL"
      kill -KILL -- "-$3" 2>/dev/null || true
    fi
  fi
  rm -f "$RUN_DIR/$1.pid"
}

wait_http() { # url seconds pid log label
  local i=0
  while [ "$i" -lt "$2" ]; do
    if http_ok "$1"; then return 0; fi
    if ! alive "$3"; then
      tail -n 40 "$4" >&2
      explain_failure "$4"
      die "$5 exited before answering $1 — log: $4"
    fi
    sleep 2
    i=$((i + 2))
  done
  tail -n 40 "$4" >&2
  explain_failure "$4"
  die "$5 not answering $1 after ${2}s (still running as pid $3; \`just down\` stops it) — log: $4"
}

# The failures that are about sharing one database, named where they show up.
explain_failure() {
  if grep -q "does not open this database" "$1" 2>/dev/null; then
    warn "every stack shares one database, so every stack must use the key its secrets were sealed with.
      OXY_ENCRYPTION_KEY here differs from the one that sealed them (another checkout's .env, or
      <state dir>/encryption_key.txt when unset). Use the same .env in every checkout."
  fi
}

# ── preflight ────────────────────────────────────────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || die "$1 not found on PATH — $2"; }

preflight_tools() {
  need python3 "used for process spawning, the slot registry and state.json"
  need curl "used for health checks"
  if [ "$FRONTEND_ONLY" != 1 ]; then
    need docker "install Docker Desktop or OrbStack"
    docker info >/dev/null 2>&1 ||
      die "Docker daemon not reachable (\`docker info\` failed). Start Docker; from Claude Code, run this outside the Bash sandbox (it needs the Docker socket)."
    [ "$NO_BUILD" = 1 ] || [ -n "${OXY_BIN:-}" ] || need cargo "install rustup"
  fi
  [ "$NO_FRONTEND" = 1 ] || need pnpm "see DEVELOPMENT.md prerequisites"
  if [ -n "${OXY_ROLE:-}" ]; then note "OXY_ROLE=$OXY_ROLE is dropped for this stack (each backend is one all-roles process)"; fi
  if [ -n "$(env_or_dotenv OXY_DEV_LOGIN_EMAILS)" ]; then
    warn "OXY_DEV_LOGIN_EMAILS is set — it replaces the persona roster, so /dev-login?as=<persona> may refuse"
  fi
  if [ -z "$(env_or_dotenv OXY_GLOBAL_ADMINS)" ]; then
    warn "OXY_GLOBAL_ADMINS is not in .env (or the env) — dev-login stays disabled, so no persona can sign in. Add e.g. OXY_GLOBAL_ADMINS=you@oxy.tech to .env"
  fi
}

# A worktree has no .env of its own. Every stack shares one database, so it must also share
# the key that seals the database's secrets: link the main checkout's .env rather than run
# with none (no dev-login roster, no LLM keys, a different encryption key).
link_env() {
  local common main
  [ ! -e "$REPO/.env" ] || return 0
  common=$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || return 0
  main=$(dirname "$common")
  if [ "$main" != "$REPO" ] && [ -f "$main/.env" ]; then
    ln -s "$main/.env" "$REPO/.env"
    note "no .env in this checkout: linked $main/.env (same database, so same keys)"
  fi
}

port_ok() { # port our-group allowed-containers(space-separated) label
  local pids pid pg names c
  if ! command -v lsof >/dev/null 2>&1; then
    # No lsof: only "is anything there", so a port of our own live group passes.
    [ -n "$2" ] && return 0
    python3 -c 'import socket,sys; s=socket.socket(); s.bind(("0.0.0.0", int(sys.argv[1])))' "$1" 2>/dev/null && return 0
    printf '%sport %s (%s) is in use (install lsof to see by what)%s\n' "$R" "$1" "$4" "$Z" >&2
    return 1
  fi
  pids=$(lsof -nP -iTCP:"$1" -sTCP:LISTEN -t 2>/dev/null | sort -u || true)
  [ -n "$pids" ] || return 0
  if [ -n "$2" ]; then
    for pid in $pids; do
      pg=$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d ' ' || true)
      if [ "$pg" = "$2" ]; then return 0; fi
    done
  fi
  if [ -n "$3" ]; then
    names=" $(docker ps --filter "publish=$1" --format '{{.Names}}' 2>/dev/null | tr '\n' ' ' || true) "
    for c in $3; do
      case "$names" in *" $c "*) return 0 ;; esac
    done
  fi
  printf '%sport %s (%s) is held by a process this stack did not start:%s\n' "$R" "$1" "$4" "$Z" >&2
  lsof -nP -iTCP:"$1" -sTCP:LISTEN 2>/dev/null | sed 's/^/    /' >&2 || true
  return 1
}

use_slot() { # "slot api internal frontend"
  read -r SLOT BACKEND_PORT INTERNAL_PORT FRONTEND_PORT <<<"$1"
  [ -n "${FRONTEND_PORT:-}" ] || die "the slot registry answered '$1'"
  BACKEND_URL="http://127.0.0.1:$BACKEND_PORT"
  FRONTEND_URL="http://127.0.0.1:$FRONTEND_PORT"
}

# This checkout's slot, moved to another one when something we did not start holds its ports.
claim_slot() {
  local excl=() bg fg bad attempt=0
  while :; do
    use_slot "$(slots claim --repo "$REPO" "${excl[@]+"${excl[@]}"}")" || exit 1
    bg=$(backend_group)
    fg=$(frontend_group)
    bad=0
    if [ "$FRONTEND_ONLY" != 1 ]; then
      port_ok "$BACKEND_PORT" "$bg" "" "oxy API" || bad=1
      port_ok "$INTERNAL_PORT" "$bg" "" "oxy internal API" || bad=1
    fi
    if [ "$NO_FRONTEND" != 1 ]; then port_ok "$FRONTEND_PORT" "$fg" "" "Vite" || bad=1; fi
    [ "$bad" = 1 ] || break
    # A pinned slot is the one asked for: there is nowhere to move to.
    [ -z "${OXY_DEV_SLOT:-}" ] ||
      die "OXY_DEV_SLOT=$SLOT is pinned and its ports are taken (above); free them, or pin another slot"
    attempt=$((attempt + 1))
    [ "$attempt" -lt 5 ] || die "the ports of $attempt slots in a row are taken; free some, or pin one with OXY_DEV_SLOT=N"
    # Our processes there would be orphaned by the move: stop them first.
    if [ -n "$bg" ]; then stop backend 30 "$bg"; fi
    if [ -n "$fg" ]; then stop frontend 10 "$fg"; fi
    note "slot $SLOT's ports are taken (above); moving this checkout to another slot"
    excl+=(--exclude "$SLOT")
  done
  note "slot $SLOT: API :$BACKEND_PORT, internal :$INTERNAL_PORT, Vite :$FRONTEND_PORT"
}

# Running stacks of OTHER checkouts, refused for what would pull the data out from under them.
guard_others() { # "what it does", e.g. "--clean wipes"
  local others
  others=$(slots others --repo "$REPO")
  [ -n "$others" ] || return 0
  printf '%s\n' "$others" | sed 's/^/    /' >&2
  if [ "$FORCE" = 1 ]; then
    warn "$1 the data layer the stacks above are using (--force)"
  else
    die "$1 the data layer the stacks above are using. Stop them (\`just down\` in each), or pass --force"
  fi
}

# ── shared data layer ────────────────────────────────────────────────────────
data_needs() {
  NEED_AIRHOUSE=0 NEED_CLICKHOUSE=0
  local var any=""
  for var in AIRHOUSE_BASE_URL AIRHOUSE_ADMIN_TOKEN AIRHOUSE_WIRE_HOST AIRHOUSE_WIRE_PORT; do
    any="$any$(env_or_dotenv "$var")"
  done
  # With AIRHOUSE_* set, that Airhouse is the one to use, as `oxy start` decides.
  if [ "$NO_AIRHOUSE" != 1 ] && [ -z "$any" ]; then NEED_AIRHOUSE=1; fi
  if [ "$(env_or_dotenv OXY_OBSERVABILITY_BACKEND)" = clickhouse ]; then NEED_CLICKHOUSE=1; fi
}

data_healthy() {
  docker exec oxy-postgres pg_isready -q -U postgres -d oxy >/dev/null 2>&1 && tcp_ok "$PG_PORT" || return 1
  if [ "$NEED_AIRHOUSE" = 1 ]; then http_ok "$AIRHOUSE_URL/healthz" || return 1; fi
  if [ "$NEED_CLICKHOUSE" = 1 ]; then http_ok "$CLICKHOUSE_URL/ping" || return 1; fi
  return 0
}

data_line() {
  local parts="postgres :$PG_PORT"
  [ "$NEED_AIRHOUSE" = 0 ] || parts="$parts, airhouse :8080/:5445"
  [ "$NEED_CLICKHOUSE" = 0 ] || parts="$parts, clickhouse :8123"
  printf '%s' "$parts"
}

ensure_data_layer() {
  step "data layer (shared by every stack)  $(data_line)"
  if [ "$CLEAN" != 1 ] && data_healthy; then
    note "reusing it"
    return 0
  fi
  # One starter at a time, machine-wide: two `oxy start --db-only` would remove each other's containers.
  OXY_DEV_NO_AIRHOUSE="$NO_AIRHOUSE" OXY_DEV_CLEAN="$CLEAN" OXY_DEV_FORCE="$FORCE" \
    python3 "$SLOTS" with-lock data-layer -- bash "$SELF" _data-layer
}

# Under the data-layer lock.
start_data_layer() {
  data_needs
  if [ "$CLEAN" != 1 ] && data_healthy; then
    note "started by another stack while this one waited; reusing it"
    return 0
  fi
  if [ "$CLEAN" = 1 ]; then
    guard_others "--clean wipes"
  elif [ -n "$(slots others --repo "$REPO")" ]; then
    warn "restarting the shared data layer: the other running stacks lose their database for a few seconds, then reconnect"
  fi
  local bad=0
  port_ok "$PG_PORT" "" "oxy-postgres" "Postgres" || bad=1
  if [ "$NEED_CLICKHOUSE" = 1 ]; then
    port_ok 8123 "" "oxy-clickhouse" "ClickHouse HTTP" || bad=1
    port_ok 9000 "" "oxy-clickhouse" "ClickHouse native" || bad=1
  fi
  # An Airhouse already answering (`just airhouse-up`) is one `oxy start` uses, not a clash.
  if [ "$NEED_AIRHOUSE" = 1 ] && ! http_ok "$AIRHOUSE_URL/healthz"; then
    port_ok 8080 "" "oxy-airhouse-cp" "Airhouse admin API" || bad=1
    port_ok 5445 "" "oxy-airhouse-dp" "Airhouse wire" || bad=1
  fi
  [ "$bad" = 0 ] || die "free the ports above (a stack you started by hand? stop it), then re-run"
  local log flags=()
  log=$(fresh_log data-layer)
  rset data-layer.log "$log"
  [ "$CLEAN" != 1 ] || flags+=(--clean)
  # `oxy start` removes every oxy-* container first, so skipping Airhouse here would take away
  # one the other stacks are using: keep it whenever one answers, even if this stack needs none.
  if [ "$NEED_AIRHOUSE" != 1 ] && ! http_ok "$AIRHOUSE_URL/healthz"; then flags+=(--no-airhouse); fi
  note "oxy start --db-only ${flags[*]+"${flags[*]}"}  (log: $(rel "$log"))"
  [ -x "$BIN" ] || die "no oxy binary at $BIN — drop --no-build, or set OXY_BIN"
  # First run pulls the Postgres, Airhouse (and ClickHouse) images.
  if ! (cd "$REPO" && env -u OXY_ROLE "$BIN" start --db-only "${flags[@]+"${flags[@]}"}") >"$log" 2>&1; then
    tail -n 40 "$log" >&2
    die "oxy start --db-only failed — log: $log"
  fi
  data_healthy || die "the data layer is up but not healthy ($(data_line)) — log: $log"
  note "ready"
}

# ── steps ────────────────────────────────────────────────────────────────────
# The shell may export RUSTC_WRAPPER=sccache: .cargo/config.toml forbids it (sccache hard-errors
# under incremental), and a wrapper that is not installed fails every rustc invocation.
cargo_build() {
  local var val drop=""
  for var in RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER; do
    val="${!var:-}"
    [ -n "$val" ] || continue
    case "$(basename "$val")" in
      sccache*) note "ignoring $var=$val for this build (.cargo/config.toml: sccache breaks incremental)" ;;
      *)
        if command -v "$val" >/dev/null 2>&1; then continue; fi
        note "ignoring $var=$val for this build (not on PATH)"
        ;;
    esac
    drop="$drop $var"
  done
  (
    for var in $drop; do unset "$var"; done
    cd "$REPO" && python3 scripts/build-lease.py cargo build -p oxy-server
  )
}

build() {
  step "build"
  if [ -n "${OXY_BIN:-}" ]; then
    note "OXY_BIN=$OXY_BIN — not building"
    return 0
  fi
  if [ ! -e "$BIN" ]; then
    note "no $(rel "$BIN") yet: a cold build is ~7 min (\`just seed-target\` copies a warm checkout's target/ in ~9s)"
  fi
  local log
  log=$(fresh_log build)
  rset build.log "$log"
  note "cargo build -p oxy-server  (log: $(rel "$log"))"
  if ! cargo_build >"$log" 2>&1; then
    grep -E '^error' "$log" | head -20 >&2 || true
    tail -n 40 "$log" >&2
    die "cargo build failed — log: $log"
  fi
}

# Where this checkout's backend (and its seed) find the shared data layer: what `oxy start`
# sets for itself, deferring to whatever the env or .env already names.
backend_env() {
  BACKEND_ENV=(
    OXY_DATABASE_URL="$DATABASE_URL"
    # Auth stays on (MAGIC_LINK_LOCAL_TEST mounts /login + /dev-login); app email previews
    # instead of hitting real SES. Both defer to a value already in the environment.
    MAGIC_LINK_LOCAL_TEST="${MAGIC_LINK_LOCAL_TEST:-true}"
    OXY_APP_EMAIL_LOCAL_TEST="${OXY_APP_EMAIL_LOCAL_TEST:-1}"
  )
  # Per-org OLTP: each org a database in this cluster, unless a provider is named.
  if [ -z "$(env_or_dotenv OXY_OLTP_PROVIDER)" ]; then
    BACKEND_ENV+=(OXY_OLTP_PROVIDER=local)
    [ -n "$(env_or_dotenv OXY_OLTP_ADMIN_URL)" ] || BACKEND_ENV+=(OXY_OLTP_ADMIN_URL="$DATABASE_URL")
  fi
  if [ "$NEED_AIRHOUSE" = 1 ]; then
    BACKEND_ENV+=(AIRHOUSE_BASE_URL="$AIRHOUSE_URL" AIRHOUSE_ADMIN_TOKEN=airhouse-local-token
      AIRHOUSE_WIRE_HOST=localhost AIRHOUSE_WIRE_PORT=5445)
  fi
  if [ "$NEED_CLICKHOUSE" = 1 ]; then
    BACKEND_ENV+=(OXY_CLICKHOUSE_URL="$CLICKHOUSE_URL" OXY_CLICKHOUSE_USER=default
      OXY_CLICKHOUSE_PASSWORD=default OXY_CLICKHOUSE_DATABASE=observability)
  fi
}

# An older `just up` ran `oxy start`, which owns the containers and removes them when it stops.
# Stop it before deciding on the data layer, so that removal happens now and not later, under
# every other stack.
retire_legacy_backend() {
  local group
  group=$(our_group backend "$LEGACY_BACKEND_MATCH")
  [ -n "$group" ] || return 0
  note "replacing the \`oxy start\` backend an older \`just up\` left here (it removes the containers it started)"
  stop backend 30 "$group"
}

backend() {
  step "backend  $BACKEND_URL"
  [ -x "$BIN" ] || die "no oxy binary at $BIN — drop --no-build, or set OXY_BIN"
  local group log pid fp
  group=$(backend_group)
  fp=$(fingerprint "$BIN")
  if [ -n "$group" ] && [ "$RESTART" != 1 ] && [ "$CLEAN" != 1 ] &&
    [ "$fp" = "$(rget backend.fingerprint)" ] && http_ok "$BACKEND_URL/api/ready"; then
    note "reusing pid $group (ready, binary unchanged)"
    return 0
  fi
  if [ -n "$group" ]; then
    if [ "$fp" != "$(rget backend.fingerprint)" ]; then note "binary changed since pid $group started"; fi
    stop backend 30 "$group"
  fi
  log=$(fresh_log backend)
  pid=$(spawn "$log" env -u OXY_ROLE "${BACKEND_ENV[@]}" \
    "$BIN" serve --enterprise --port "$BACKEND_PORT" --internal-port "$INTERNAL_PORT") ||
    die "could not launch $BIN"
  rset backend.pid "$pid"
  rset backend.fingerprint "$fp"
  rset backend.log "$log"
  note "oxy serve --enterprise  pid $pid  (log: $(rel "$log"))"
  wait_http "$BACKEND_URL/api/ready" "${OXY_DEV_UP_READY_SECS:-240}" "$pid" "$log" "backend"
  note "ready"
}

seed() {
  step "seed  (shared data: one seed at a time, machine-wide)"
  local log
  log=$(fresh_log seed)
  rset seed.log "$log"
  # --llm-keys: the database is this machine's shared container, so this box's keys stay on it.
  # --fixtures: the pulse-demo fixture app (release history + traffic); a no-op once built.
  note "oxy dev seed --workspace-path ./examples --llm-keys --fixtures  (log: $(rel "$log"))"
  if ! (cd "$REPO" && python3 "$SLOTS" with-lock --log "$log" seed -- env -u OXY_ROLE "${BACKEND_ENV[@]}" \
    "$BIN" dev seed --workspace-path ./examples --llm-keys --fixtures); then
    tail -n 40 "$log" >&2
    die "oxy dev seed failed — log: $log"
  fi
}

# --frontend-only: a backend some checkout on this machine already runs.
pick_backend() {
  step "backend  (shared: --frontend-only)"
  local line from url
  if [ -n "${OXY_DEV_BACKEND_URL:-}" ]; then
    url="${OXY_DEV_BACKEND_URL%/}" from="url"
    http_ok "$url/api/ready" || die "OXY_DEV_BACKEND_URL=$url does not answer /api/ready"
  else
    line=$(slots backends --repo "$REPO" --first)
    [ -n "$line" ] || die "--frontend-only needs a backend, and none is running on this machine. Run \`just up\` (without it) here or in another checkout"
    read -r from url <<<"$line"
  fi
  BACKEND_URL="$url"
  # A registry URL is always http://127.0.0.1:<port>; proxy as this checkout's own backend
  # would (localhost). A URL someone named goes through as given.
  if [ "$from" = url ]; then PROXY_TARGET="$url"; else PROXY_TARGET="http://localhost:${url##*:}"; fi
  if [ "$from" = "$SLOT" ]; then
    BACKEND_FROM=""
    note "this checkout's own backend, $url"
  else
    BACKEND_FROM="$from"
    note "using $([ "$from" = url ] && echo "$url" || echo "slot $from's backend, $url") — its binary, not this checkout's"
  fi
  rset backend_from "$BACKEND_FROM"
  rset backend_url "$BACKEND_URL"
}

# node_modules/.pnpm/lock.yaml is pnpm's copy of the lockfile it last installed. Behind the
# real one, a later `pnpm run`/`exec` may install implicitly (or run against stale deps), so
# install explicitly, frozen, where the failure is visible.
deps_stale() {
  local marker="$REPO/node_modules/.pnpm/lock.yaml"
  [ -d "$REPO/web-app/node_modules" ] || return 0
  [ -f "$marker" ] || return 0
  [ "$REPO/pnpm-lock.yaml" -nt "$marker" ] && ! cmp -s "$REPO/pnpm-lock.yaml" "$marker"
}

frontend() {
  step "frontend  $FRONTEND_URL  -> $PROXY_TARGET"
  local group log pid installed=0
  group=$(frontend_group)
  if deps_stale; then
    if [ -n "$group" ]; then stop frontend 10 "$group"; fi
    group=""
    log=$(fresh_log pnpm-install)
    note "node_modules missing or behind pnpm-lock.yaml — pnpm install --frozen-lockfile  (log: $(rel "$log"))"
    (cd "$REPO" && pnpm install --frozen-lockfile --config.confirmModulesPurge=false) >"$log" 2>&1 || {
      tail -n 40 "$log" >&2
      die "pnpm install failed — log: $log"
    }
    installed=1
  fi
  if [ -n "$group" ] && [ "$RESTART" != 1 ] && [ "$installed" = 0 ] &&
    [ "$(rget frontend.target)" = "$PROXY_TARGET" ] && http_ok "$FRONTEND_URL/"; then
    note "reusing pid $group"
  else
    if [ -n "$group" ]; then stop frontend 10 "$group"; fi
    log=$(fresh_log frontend)
    pid=$(spawn "$log" env OXY_DEV_HOST=127.0.0.1 OXY_DEV_PORT="$FRONTEND_PORT" \
      OXY_DEV_PROXY_TARGET="$PROXY_TARGET" \
      pnpm --dir web-app run dev "$FRONTEND_MATCH") || die "could not launch pnpm"
    rset frontend.pid "$pid"
    rset frontend.log "$log"
    rset frontend.target "$PROXY_TARGET"
    note "vite  pid $pid  (log: $(rel "$log"))"
    wait_http "$FRONTEND_URL/" 90 "$pid" "$log" "Vite"
  fi
  http_ok "$FRONTEND_URL/api/health" || warn "Vite is up but its /api proxy does not reach the backend"
}

record() { # tell the machine-wide registry what this checkout now runs
  local bpid fpid args=(--repo "$REPO")
  bpid=$(backend_group)
  fpid=$(frontend_group)
  if [ -n "$bpid" ]; then args+=(--backend "$bpid"); fi
  if [ -n "$fpid" ]; then args+=(--frontend "$fpid"); fi
  if [ -n "${BACKEND_FROM:-}" ] && [ "$BACKEND_FROM" != url ]; then args+=(--backend-from "$BACKEND_FROM"); fi
  slots touch "${args[@]}"
}

write_state() {
  local fe_log="" fe_pid="" be_pid="" internal="http://127.0.0.1:$INTERNAL_PORT"
  if [ -n "$(frontend_group)" ]; then
    fe_pid=$(rget frontend.pid)
    fe_log=$(rget frontend.log)
  fi
  if [ -z "${BACKEND_FROM:-}" ]; then be_pid=$(rget backend.pid); else internal=""; fi
  STATE_JSON="$STATE_JSON" REPO="$REPO" BIN="$BIN" DATABASE_URL="$DATABASE_URL" \
    SLOT="$SLOT" BACKEND_URL="$BACKEND_URL" INTERNAL_URL="$internal" \
    FRONTEND_URL="$FRONTEND_URL" BACKEND_FROM="${BACKEND_FROM:-}" \
    AIRHOUSE_URL="$([ "${NEED_AIRHOUSE:-0}" = 1 ] && echo "$AIRHOUSE_URL")" \
    BACKEND_PID="$be_pid" FRONTEND_PID="$fe_pid" \
    BACKEND_LOG="$(rget backend.log)" FRONTEND_LOG="$fe_log" SEED_LOG="$(rget seed.log)" \
    BUILD_LOG="$(rget build.log)" STAFF_EMAILS="$(env_or_dotenv OXY_GLOBAL_ADMINS)" \
    python3 "$STATE_WRITER"
}

data_status() {
  local line
  line=$(docker ps --format '{{.Names}}: {{.Status}}' 2>/dev/null |
    grep -E '^oxy-(postgres|clickhouse|airhouse-cp|airhouse-dp|airhouse-minio):' | tr '\n' ' ' || true)
  printf '%s\n' "${line:-none running}"
}

status() {
  local line bpid fpid code=0 fp from
  line=$(slots show --repo "$REPO")
  if [ -z "$line" ]; then
    echo "no stack here yet — \`just up\` claims a slot (\`just stacks\` shows the machine)"
    return 1
  fi
  use_slot "$line"
  printf '%sslot%s      %s  (API :%s, internal :%s, Vite :%s)\n' "$B" "$Z" "$SLOT" "$BACKEND_PORT" "$INTERNAL_PORT" "$FRONTEND_PORT"
  bpid=$(backend_group)
  fpid=$(frontend_group)
  from=$(rget backend_from)
  printf '%sbackend%s   ' "$B" "$Z"
  if [ -n "$from" ] && [ -z "$bpid" ]; then
    BACKEND_URL=$(rget backend_url)
    [ "$from" = url ] && from=OXY_DEV_BACKEND_URL || from="slot $from"
    if http_ok "$BACKEND_URL/api/ready"; then echo "$from's  $BACKEND_URL/api/ready ok"; else
      echo "$from's  $BACKEND_URL/api/ready NOT ready"
      code=1
    fi
  elif [ -z "$bpid" ]; then
    echo "not running (nothing recorded by just up)"
    code=1
  elif http_ok "$BACKEND_URL/api/ready"; then
    fp="binary unchanged"
    [ "$(fingerprint "$BIN")" = "$(rget backend.fingerprint)" ] || fp="binary REBUILT since start — \`just up\` restarts it"
    echo "pid $bpid  $BACKEND_URL/api/ready ok  ($fp)"
  else
    echo "pid $bpid  $BACKEND_URL/api/ready NOT ready"
    code=1
  fi
  printf '%sfrontend%s  ' "$B" "$Z"
  if [ -z "$fpid" ]; then
    echo "not running"
    code=1
  elif http_ok "$FRONTEND_URL/"; then
    if http_ok "$FRONTEND_URL/api/health"; then echo "pid $fpid  $FRONTEND_URL ok (proxy ok)"; else echo "pid $fpid  $FRONTEND_URL ok, /api proxy FAILING"; fi
  else
    echo "pid $fpid  $FRONTEND_URL NOT answering"
    code=1
  fi
  printf '%sdata%s      %s\n' "$B" "$Z" "$(data_status)"
  local others name path
  others=$(slots others --repo "$REPO" | wc -l | tr -d ' ')
  printf '%sstacks%s    %s other running on this machine (%s)\n' "$B" "$Z" "$others" "\`just stacks\`"
  for name in build data-layer backend seed frontend; do
    path=$(rget "$name.log")
    if [ -n "$path" ]; then printf '%slog%s       %-10s %s\n' "$B" "$Z" "$name" "$(rel "$path")"; fi
  done
  if [ -f "$STATE_JSON" ]; then printf '%sstate%s     %s\n' "$B" "$Z" "$(rel "$STATE_JSON")"; fi
  return "$code"
}

stacks() {
  printf '%sdata layer%s  %s\n' "$B" "$Z" "$(data_status)"
  printf '%sstacks%s      (registry: %s)\n' "$B" "$Z" "${OXY_DEV_HOME:-${XDG_STATE_HOME:-~/.local/state}/oxy-dev}"
  slots list --repo "$REPO"
}

down() {
  local line
  if [ "$DB" = 1 ]; then guard_others "--db stops"; fi
  line=$(slots show --repo "$REPO")
  if [ -n "$line" ]; then use_slot "$line"; fi
  stop frontend 10 "$(frontend_group)"
  stop backend 30 "$(backend_group)"
  rm -f "$STATE_JSON" "$RUN_DIR/backend_from" "$RUN_DIR/backend_url"
  if [ "$RELEASE" = 1 ]; then
    slots release --repo "$REPO"
    note "slot released"
  elif [ -n "$line" ]; then
    slots touch --repo "$REPO"
  fi
  if [ "$DB" = 1 ]; then
    local running c
    running=" $(docker ps --format '{{.Names}}' 2>/dev/null | tr '\n' ' ' || true) "
    for c in $DATA_CONTAINERS; do
      case "$running" in
        *" $c "*)
          note "docker stop $c"
          docker stop "$c" >/dev/null
          ;;
      esac
    done
  fi
  note "stopped (logs kept in $(rel "$LOG_DIR"))"
}

up() {
  link_env
  preflight_tools
  claim_slot
  data_needs
  BACKEND_FROM=""
  PROXY_TARGET="http://localhost:$BACKEND_PORT"
  if [ "$FRONTEND_ONLY" = 1 ]; then
    pick_backend
  else
    rm -f "$RUN_DIR/backend_from" "$RUN_DIR/backend_url"
    [ "$NO_BUILD" = 1 ] || build
    retire_legacy_backend
    ensure_data_layer
    backend_env
    backend
    [ "$NO_SEED" = 1 ] || seed
  fi
  [ "$NO_FRONTEND" = 1 ] || frontend
  record
  step "state"
  write_state
}

usage() { sed -n '2,34p' "$0" | sed -E 's/^# ?//'; }

# ── main ─────────────────────────────────────────────────────────────────────
CMD=up
case "${1:-}" in
  up | down | status | stacks | help | _data-layer) CMD="$1" && shift ;;
esac
NO_BUILD=0 NO_SEED=0 NO_FRONTEND=0 FRONTEND_ONLY=0 RESTART=0 CLEAN=0 DB=0 RELEASE=0 FORCE=0 NO_AIRHOUSE=0
if [ "$CMD" = _data-layer ]; then # re-entered by ensure_data_layer under the machine-wide lock
  CLEAN="${OXY_DEV_CLEAN:-0}" FORCE="${OXY_DEV_FORCE:-0}" NO_AIRHOUSE="${OXY_DEV_NO_AIRHOUSE:-0}"
fi
for arg in "$@"; do
  case "$arg" in
    --no-build) NO_BUILD=1 ;;
    --no-seed) NO_SEED=1 ;;
    --no-frontend) NO_FRONTEND=1 ;;
    --frontend-only) FRONTEND_ONLY=1 ;;
    --restart) RESTART=1 ;;
    --clean) CLEAN=1 ;;
    --no-airhouse) NO_AIRHOUSE=1 ;;
    --db) DB=1 ;;
    --release) RELEASE=1 ;;
    --force) FORCE=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) die "unknown argument: $arg (commands: up | down | status | stacks; flags: see --help)" ;;
  esac
done
if [ "$FRONTEND_ONLY" = 1 ] && [ "$NO_FRONTEND" = 1 ]; then die "--frontend-only and --no-frontend leave nothing to run"; fi
if [ "$FRONTEND_ONLY" = 1 ] && [ "$CLEAN" = 1 ]; then die "--clean wipes the shared data layer, which --frontend-only never touches"; fi
mkdir -p "$RUN_DIR" "$LOG_DIR"
# Without ps every recorded pid reads as foreign, and `up` would fight its own stack for ports.
ps -o pid= -p $$ >/dev/null 2>&1 ||
  die "\`ps\` is not permitted in this shell (a sandbox?) — dev-up.sh inspects and signals processes; run it unsandboxed"

case "$CMD" in
  up | down)
    # One `up`/`down` per checkout at a time; a second one waits, then finds the stack up.
    if [ -z "${OXY_DEV_LOCKED:-}" ]; then
      OXY_DEV_LOCKED=1 exec python3 "$SLOTS" with-lock "$DEV_DIR/up.lock" -- bash "$SELF" "$CMD" "$@"
    fi
    "$CMD"
    ;;
  _data-layer) start_data_layer ;;
  status) status ;;
  stacks) stacks ;;
  help) usage ;;
esac
