#!/usr/bin/env bash
# Boot a seeded Oxy for the release showcase (internal-docs/release-showcase.md)
# and write where it answers to $GITHUB_ENV: OXY_BASE_URL (what the browser
# opens) and OXY_BACKEND_URL (where dev-login is asked).
#
#   showcase-boot.sh image <ref>                 a published image, web-app embedded — a release,
#                                                which is where pictures are taken
#   showcase-boot.sh image <ref> --spa-checkout  that image's API behind THIS checkout's web-app,
#                                                built and served by `vite preview` — a preview of
#                                                one PR, run by hand
#   showcase-boot.sh binary                      `oxy` on PATH, web-app embedded — a local build;
#                                                no workflow uses it
#   showcase-boot.sh stop                        stop whichever of the above is running, keep logs
#
# The shape is the custom-app canary's (ci.yaml): `oxy serve --enterprise`
# against the job's Postgres service, then `oxy seed` for the demo data. Needs
# OXY_DATABASE_URL, OXY_GLOBAL_ADMINS and OXY_DEV_LOGIN_EMAILS in the env.
set -euo pipefail

MODE="${1:?usage: showcase-boot.sh binary | image <ref> [--spa-checkout] | stop}"
IMAGE="${2:-}"
SPA="${3:-}"
WORK="${GITHUB_WORKSPACE:-$(pwd)}"
EXAMPLES="${SHOWCASE_EXAMPLES:-$WORK/examples}"
TMP="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
LOG_DIR="$TMP/showcase-logs"
# Outside the checkout: inside a git repo, workspace git tooling walks up to its .git.
STATE_DIR="$TMP/oxy-state"
CONTAINER=oxy-showcase
# 3000 on a runner. Locally, beside a dev server that already holds 3000/3001,
# set SHOWCASE_PORT; the internal API takes the port after it.
PORT="${SHOWCASE_PORT:-3000}"
BACKEND="http://127.0.0.1:$PORT"
SERVE=(oxy serve --enterprise --port "$PORT" --internal-port "$((PORT + 1))")
mkdir -p "$LOG_DIR" "$STATE_DIR"

# A container sees the examples and the state dir at the SAME absolute paths
# as the host, so the workspace path the seed records resolves for the server.
DOCKER=(--network host -v "$EXAMPLES:$EXAMPLES" -v "$STATE_DIR:$STATE_DIR" -e "OXY_STATE_DIR=$STATE_DIR")
for v in OXY_DATABASE_URL OXY_GLOBAL_ADMINS OXY_DEV_LOGIN_EMAILS MAGIC_LINK_LOCAL_TEST OXY_APP_EMAIL_LOCAL_TEST; do
  DOCKER+=(-e "$v")
done

server_alive() {
  if [[ "$MODE" == binary ]]; then
    kill -0 "$(cat "$LOG_DIR/oxy-server.pid")" 2>/dev/null
  else
    [[ "$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null)" == true ]]
  fi
}

server_log_tail() {
  if [[ "$MODE" == binary ]]; then tail -n 80 "$LOG_DIR/oxy-server.log"; else docker logs --tail 80 "$CONTAINER" 2>&1; fi
}

wait_for() { # wait_for <url> <label>
  for i in $(seq 1 80); do
    if curl -sf --max-time 5 "$1" >/dev/null; then
      echo "$2 ready after ~$((i * 3))s"
      return 0
    fi
    if [[ "$2" == oxy ]] && ! server_alive; then
      echo "::error::oxy exited before it was ready"
      server_log_tail
      exit 1
    fi
    sleep 3
  done
  echo "::error::$2 did not answer $1 after 80 tries (3s apart, 5s each)"
  [[ "$2" == oxy ]] && server_log_tail
  exit 1
}

run_oxy() { # run_oxy <args…> — one-shot, to completion
  if [[ "$MODE" == binary ]]; then
    OXY_STATE_DIR="$STATE_DIR" oxy "$@"
  else
    docker run --rm "${DOCKER[@]}" "$IMAGE" oxy "$@"
  fi
}

# `oxy migrate` first — the chart's pre-upgrade hook, so the same step prod
# runs. `oxy serve` also migrates, but it answers /api/ready while it still
# is, and a seed started on "ready" then wrote into a schema that did not
# exist yet ("relation organizations does not exist"): a race the boot lost
# once the image pull made it slow enough to see.
migrate() {
  [[ "$MODE" == image ]] && docker pull --quiet "$IMAGE" >/dev/null
  run_oxy migrate 2>&1 | tee "$LOG_DIR/migrate.log"
}

start_server() {
  if [[ "$MODE" == binary ]]; then
    OXY_STATE_DIR="$STATE_DIR" nohup "${SERVE[@]}" >"$LOG_DIR/oxy-server.log" 2>&1 &
    echo "$!" >"$LOG_DIR/oxy-server.pid"
  else
    docker run -d --name "$CONTAINER" "${DOCKER[@]}" "$IMAGE" "${SERVE[@]}" >/dev/null
  fi
  wait_for "$BACKEND/api/ready" oxy
}

# After `migrate`: the schema the seed writes into is complete.
seed() {
  local cmd=(oxy seed --workspace-path "$EXAMPLES")
  if [[ "$MODE" == binary ]]; then
    OXY_STATE_DIR="$STATE_DIR" "${cmd[@]}" 2>&1 | tee "$LOG_DIR/seed.log"
  else
    docker run --rm "${DOCKER[@]}" "$IMAGE" "${cmd[@]}" 2>&1 | tee "$LOG_DIR/seed.log"
  fi
}

serve_checkout_spa() {
  (cd "$WORK/web-app" && pnpm exec vite build >"$LOG_DIR/vite-build.log" 2>&1) || {
    echo "::error::vite build failed"
    tail -n 60 "$LOG_DIR/vite-build.log"
    exit 1
  }
  # `vite preview` proxies /api like the dev server (preview.proxy defaults to
  # server.proxy), and vite.config.ts reads the target from the env.
  (cd "$WORK/web-app" && OXY_DEV_PROXY_TARGET="$BACKEND" nohup pnpm exec vite preview \
    --host 127.0.0.1 --port 4173 --strictPort >"$LOG_DIR/vite-preview.log" 2>&1 &
    echo "$!" >"$LOG_DIR/vite.pid")
  wait_for http://127.0.0.1:4173/ "vite preview"
}

stop() {
  [[ -f "$LOG_DIR/vite.pid" ]] && kill "$(cat "$LOG_DIR/vite.pid")" 2>/dev/null || true
  if [[ -f "$LOG_DIR/oxy-server.pid" ]]; then kill -TERM "$(cat "$LOG_DIR/oxy-server.pid")" 2>/dev/null || true; fi
  if docker inspect "$CONTAINER" >/dev/null 2>&1; then
    docker logs "$CONTAINER" >"$LOG_DIR/oxy-server.log" 2>&1 || true
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
}

case "$MODE" in
  stop) stop; exit 0 ;;
  binary) ;;
  image) [[ -n "$IMAGE" ]] || { echo "::error::image mode needs an image ref"; exit 1; } ;;
  *) echo "::error::unknown mode '$MODE'"; exit 1 ;;
esac

migrate
start_server
seed
BASE="$BACKEND"
if [[ "$SPA" == --spa-checkout ]]; then
  serve_checkout_spa
  BASE=http://127.0.0.1:4173
fi
{
  echo "OXY_BASE_URL=$BASE"
  echo "OXY_BACKEND_URL=$BACKEND"
} >>"${GITHUB_ENV:-/dev/null}"
echo "showcase instance: browser at $BASE, API at $BACKEND"
