#!/usr/bin/env bash
#
# The shared fixture layer the browser flows run against.
#
# ── Why this file exists ────────────────────────────────────────────────────
# Eleven flows under web-app/tests/agentic/flows/ could not pass, and not one
# of them failed because the product was broken. They failed because nothing
# stood up the data they read:
#
#   • the four observability flows drive `/ide/observability/*`, which needs a
#     WORKSPACE to render — the server behind them answered `/api/orgs` with
#     `[]`, so there was no project and every case timed out on its first
#     locator, a failure shape indistinguishable from a real bug;
#   • `customer-apps-oxy-starter-fleet` opens a PUBLISHED app;
#   • `partners-console-fleet` needs a partner org (Acme Consulting) with
#     managed clients, operators and a ceiling;
#   • `chat-ask-fleet` / `context-graph-fleet` / `ide-unavailable-fleet` need a
#     compiled + PROMOTED revision, or every compiled read 503s.
#
# All of that already exists as one first-party command — `oxy dev seed`, which
# documents itself as idempotent (crates/server/src/dev/seed/seed.rs:14) and
# folds in the partner tenants, the platform grants and the example-app deploy.
# What did not exist was (a) anything that ran it for the flows, (b) anything
# that ran `compile --promote` after it, and (c) any check that the fixtures are
# actually VISIBLE over HTTP from the node a flow will drive. (c) is the part
# that matters: a seed that writes rows nobody can read still fails every flow,
# just three minutes later and looking like a product defect.
#
# ── Why not extend the runner's setup surface ───────────────────────────────
# `web-app/tests/agentic/fixtures/reset.ts` is deliberately three commands wide
# (`goto:`, `reset_test_file`, `restore_demo_file:`) and must not make network
# calls — see the 2026-05-06 incident note in tests/agentic/README.md. So
# seeding lives out here in the shell, the same shape `scripts/seed-observability.sh`
# uses, and the runner keeps knowing nothing about databases.
#
# ── Usage ───────────────────────────────────────────────────────────────────
#   scripts/seed-fixtures.sh fleet     # docker fleet: seed via the ide container,
#                                      # then verify against a stateless replica
#   scripts/seed-fixtures.sh native    # a locally-running `oxy serve` / `oxy start`:
#                                      # seed via ./target/debug/oxy against its DB
#   scripts/seed-fixtures.sh check     # verify only — no writes at all
#
#   OXY_BASE_URL     where to VERIFY (default: http://127.0.0.1:3000)
#   OXY_DATABASE_URL native mode: which DB to seed (default: read out of .env)
#   FIXTURE_FLOW_EMAIL  / FIXTURE_STAFF_EMAIL  identities to mint sessions for
#
# Idempotent by construction: `oxy dev seed` skips rows it already wrote, `compile`
# writes a fresh revision and re-points the pointer (a new row, never a 409),
# and the MinIO bucket is created with `--ignore-existing`. Running it twice
# leaves the same fixture and the same PASS lines.
#
# ── Exit codes ──────────────────────────────────────────────────────────────
#   0  seeded and every fixture verified over HTTP
#   1  seeded, but a fixture is missing from the node the flows will drive
#      (a real gap — fix it, do not run flows against it and blame the product)
#   2  could not seed at all (no server, no binary, no database)
set -uo pipefail

cd "$(dirname "$0")/.."
REPO="$PWD"

MODE="${1:-check}"
case "$MODE" in
  fleet | native | check) ;;
  -h | --help)
    sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    printf 'unknown target: %s (want fleet | native | check)\n' "$MODE" >&2
    exit 2
    ;;
esac

BASE="${OXY_BASE_URL:-http://127.0.0.1:3000}"
BASE="${BASE%/}"
# Deterministic: Uuid::new_v5(NAMESPACE_DNS, "demo.oxy.local") — seed.rs:38-40.
# Same constant scripts/fleet-assert.sh pins, deliberately not re-derived.
WS="${WS:-70787bb2-e11b-5488-b2c3-02e60d5fc7d3}"
# The Local org is the nil UUID by convention (airhouse::LOCAL_ORG_ID).
ORG_ID="${ORG_ID:-00000000-0000-0000-0000-000000000000}"

# TWO identities, and mixing them up is the single most expensive mistake here.
#
# `flow@oxy.local` is an ordinary org owner with NO platform standing. Every
# WORKSPACE-scoped flow must sign in as this one: the SPA bounces a Global Owner
# off every tenant workspace URL into /admin until they open an assume-role
# session, so a workspace flow signed in as staff lands on the admin console and
# times out waiting for a testid that was never going to render.
#
# `e2e@oxy.tech` is staff (OXY_OWNER). The admin + partner surfaces need it, and
# only it — `/api/customer-apps` answers a non-staff caller 403, measured.
#
# The staff address is a property of the DEPLOYMENT, not of this script, so its
# default follows the target rather than being one hardcoded value that is wrong
# half the time: the docker fleet sets `OXY_OWNER: e2e@oxy.tech` in
# docker-compose.fleet.yml, while a native `oxy serve` takes OXY_OWNER out of
# `.env` (hello@oxy.tech here). Getting this wrong reports a perfectly good
# fixture as missing — which it did once, before this branch existed.
FLOW_EMAIL="${FIXTURE_FLOW_EMAIL:-flow@oxy.local}"
default_staff_email() {
  [ "$MODE" = fleet ] && {
    printf 'e2e@oxy.tech'
    return
  }
  local owner
  owner=$(grep -E '^OXY_OWNER=' .env 2>/dev/null | head -1 | cut -d= -f2- | tr -d '"'\''' || true)
  printf '%s' "${owner:-e2e@oxy.tech}"
}
STAFF_EMAIL="${FIXTURE_STAFF_EMAIL:-$(default_staff_email)}"

PASS=0
FAIL=0
ok() {
  PASS=$((PASS + 1))
  printf '  \033[32m✓\033[0m %s\n' "$1"
}
bad() {
  FAIL=$((FAIL + 1))
  printf '  \033[31m✗\033[0m %s\n' "$1"
}
step() { printf '\n\033[1m%s\033[0m\n' "$1"; }
die() {
  printf '\033[31mfatal:\033[0m %s\n' "$1" >&2
  exit 2
}

COMPOSE=(docker compose -f docker-compose.fleet.yml)

# `.env` is dotenvx-shaped and NOT shell-sourceable (`source .env` is a syntax
# error). Pull single values out instead.
envval() { grep -E "^$1=" .env 2>/dev/null | head -1 | cut -d= -f2- | tr -d '"'\''' || true; }

# ── Seed ─────────────────────────────────────────────────────────────────────
# Both branches run the same two commands in the same order. `seed` creates the
# org + workspace ROW; `compile --promote` is what makes that row readable —
# without a promoted revision every compiled read answers 503 on a replica and
# the flow sees an empty page, not an error.
#
# OXY_GLOBAL_ADMINS is passed to the SEED PROCESS ONLY, never exported to the
# server. `bind_org_admin_emails` reads it to bind those addresses as Owner of
# the Local org (seed.rs:44-50) — org membership, not platform standing. Export
# it to the server too and `flow@oxy.local` becomes staff, which is exactly the
# identity mistake described above.
seed_fleet() {
  step "1. Seed the fleet (inside the ide container — the only node with the files)"
  "${COMPOSE[@]}" ps --format '{{.Name}}' 2>/dev/null | grep -q 'oxy-fleet-ide' \
    || die "the fleet is not up — docker compose -f docker-compose.fleet.yml up -d"

  # The working copy. NOT removed first if it is already there: it is the
  # workspace other tooling reads, and `compile` re-reads it from scratch
  # anyway, so a re-copy would be churn with a delete in front of it.
  if "${COMPOSE[@]}" exec -T ide test -d /var/lib/oxy/data/examples 2>/dev/null; then
    ok "ide already holds /var/lib/oxy/data/examples"
  else
    "${COMPOSE[@]}" cp examples ide:/var/lib/oxy/data/examples >/dev/null 2>&1 \
      || die "could not copy examples/ into the ide container"
    ok "copied examples/ into the ide container"
  fi

  "${COMPOSE[@]}" exec -T -e OXY_GLOBAL_ADMINS="${FLOW_EMAIL},dev@oxy.local" \
    -w /var/lib/oxy/data ide oxy dev seed >/tmp/oxy-seed-fixtures-seed.log 2>&1 \
    || die "oxy dev seed failed inside the ide container (see /tmp/oxy-seed-fixtures-seed.log)"
  ok "oxy dev seed: $(grep -cE '^  ✓' /tmp/oxy-seed-fixtures-seed.log) fixtures reported"

  "${COMPOSE[@]}" exec -T ide oxy dev compile --workspace-path /var/lib/oxy/data/examples \
    --workspace-id "$WS" --enterprise --promote --skip-migrations \
    >/tmp/oxy-seed-fixtures-compile.log 2>&1 \
    || die "compile failed inside the ide container (see /tmp/oxy-seed-fixtures-compile.log)"
  ok "compiled + promoted ($(grep -oE 'revision_id  = [0-9a-f-]+' /tmp/oxy-seed-fixtures-compile.log | tail -1))"
}

seed_native() {
  step "1. Seed a locally-running server's database"
  local bin="${OXY_BIN:-$REPO/target/debug/oxy}"
  [ -x "$bin" ] || die "no oxy binary at $bin — run: cargo build"

  local db="${OXY_DATABASE_URL:-$(envval OXY_DATABASE_URL)}"
  [ -n "$db" ] || die "no OXY_DATABASE_URL (not in the environment, not in .env)"
  printf '  database: %s\n' "${db##*@}"

  OXY_DATABASE_URL="$db" OXY_GLOBAL_ADMINS="${FLOW_EMAIL},dev@oxy.local" \
    "$bin" dev seed --workspace-path "$REPO/examples" >/tmp/oxy-seed-fixtures-seed.log 2>&1 \
    || die "oxy dev seed failed (see /tmp/oxy-seed-fixtures-seed.log)"
  ok "oxy dev seed: $(grep -cE '^  ✓' /tmp/oxy-seed-fixtures-seed.log) fixtures reported"

  OXY_DATABASE_URL="$db" "$bin" dev compile --workspace-path "$REPO/examples" \
    --workspace-id "$WS" --enterprise --promote --skip-migrations \
    >/tmp/oxy-seed-fixtures-compile.log 2>&1 \
    || die "compile failed (see /tmp/oxy-seed-fixtures-compile.log)"
  ok "compiled + promoted ($(grep -oE 'revision_id  = [0-9a-f-]+' /tmp/oxy-seed-fixtures-compile.log | tail -1))"
}

# ── Verify ───────────────────────────────────────────────────────────────────
# Every check below is an HTTP request against $BASE — the SAME node and the
# SAME routes a flow drives, not a SQL query against the database the seed just
# wrote. A row that exists but 403s, 404s or answers off an unpromoted revision
# is indistinguishable from no row at all from where the browser sits, and that
# is precisely the failure this file exists to make loud.
mint() { # email -> writes /tmp/oxy-seed-fixtures-<slug>.json, echoes the token
  local email="$1" slug
  slug=$(printf '%s' "$email" | tr -c 'a-zA-Z0-9' '-')
  curl -s --max-time 10 "${BASE}/api/auth/dev-login?email=${email}" \
    -o "/tmp/oxy-seed-fixtures-${slug}.json" 2>/dev/null
  python3 -c 'import json,sys
try: print(json.load(open(sys.argv[1])).get("token",""))
except Exception: print("")' "/tmp/oxy-seed-fixtures-${slug}.json"
}

# Every check below prints the BODY it judged on failure. A check that only
# says "expected acme" costs another five minutes finding out what it did see.
# `jq -e` exits non-zero on a false/null result AND on unparseable input, so a
# 403 HTML body fails the same way an empty list does — which is correct here.
has() { # json-file  jq-filter  [jq args…]
  local f="$1" q="$2"
  shift 2
  jq -e "$@" "$q" "$f" >/dev/null 2>&1
}

verify() {
  step "2. Verify every fixture over HTTP at $BASE"
  curl -sf --max-time 8 "${BASE}/api/health" >/dev/null 2>&1 \
    || die "nothing healthy at ${BASE}/api/health"

  local tok stok body code
  tok=$(mint "$FLOW_EMAIL")
  if [ -z "$tok" ]; then
    bad "dev-login as $FLOW_EMAIL returned no token — is it in OXY_DEV_LOGIN_EMAILS?"
    return
  fi
  ok "dev-login as $FLOW_EMAIL (the non-staff identity workspace flows must use)"

  body=/tmp/oxy-seed-fixtures-check.json

  # (a) the org exists AND the flow identity is a member of it. Both halves
  #     matter: `GET /orgs` is membership-scoped, so a seeded org the flow
  #     identity was never bound to reads exactly like no org at all.
  curl -s --max-time 10 -H "Authorization: Bearer $tok" "${BASE}/api/orgs" -o "$body"
  if has "$body" 'any(.[]; .slug == "local")'; then
    ok "org 'local' visible to $FLOW_EMAIL"
  else
    bad "org 'local' NOT visible to $FLOW_EMAIL — got: $(head -c 200 "$body")"
  fi

  # (b) the workspace row.
  curl -s --max-time 10 -H "Authorization: Bearer $tok" "${BASE}/api/orgs/${ORG_ID}/workspaces" -o "$body"
  if has "$body" 'any(.[]; .id == $ws)' --arg ws "$WS"; then
    ok "workspace $WS listed under org 'local'"
  else
    bad "workspace $WS missing — got: $(head -c 200 "$body")"
  fi

  # (c) the workspace is COMPILED AND PROMOTED, proven from wherever $BASE is.
  #     `/agents` is FleetOk and reads the promoted revision, so a stateless
  #     replica answering it with real agents is the strongest single signal
  #     that the compile boundary has something to serve. An unpromoted
  #     workspace answers here, and only here, with a 503 or an empty list.
  code=$(curl -s --max-time 15 -o "$body" -w '%{http_code}' \
    -H "Authorization: Bearer $tok" "${BASE}/api/${WS}/agents")
  if [ "$code" = 200 ] && has "$body" 'length > 0'; then
    ok "workspace has a promoted revision ($(jq -r 'length' "$body") agents readable at $BASE)"
  else
    bad "no readable compiled revision — /api/$WS/agents => $code $(head -c 160 "$body")"
  fi

  # (d) the published custom app, fetched exactly the way the browser fetches
  #     it. NOT the admin listing: a row in `apps` says the app was published,
  #     it does not say the BYTES are reachable from this node. That gap is
  #     real and was measured on this very fixture — see the build-store note
  #     in docker-compose.fleet.yml.
  code=$(curl -s --max-time 15 -o "$body" -w '%{http_code}' \
    --cookie "oxy_session=$tok" "${BASE}/customer-apps/local/oxy-starter/")
  if [ "$code" = 200 ] && grep -q '__OXY_APP__' "$body"; then
    ok "custom app serves at ${BASE}/customer-apps/local/oxy-starter/ with app identity injected"
  elif [ "$code" = 200 ]; then
    bad "custom app served 200 but window.__OXY_APP__ was never injected"
  else
    bad "custom app NOT reachable — GET /customer-apps/local/oxy-starter/ => $code"
  fi

  # (e) staff-only fixtures. A deployment may legitimately not allow the staff
  #     address to sign in (it is a separate OXY_DEV_LOGIN_EMAILS entry), so a
  #     missing token here reports as a gap against the two flows that need it
  #     rather than failing the whole run.
  stok=$(mint "$STAFF_EMAIL")
  if [ -z "$stok" ]; then
    bad "dev-login as $STAFF_EMAIL returned no token — admin-airhouse-fleet and partners-console-fleet cannot run"
    return
  fi
  ok "dev-login as $STAFF_EMAIL (the staff identity admin/partner flows must use)"

  curl -s --max-time 10 -H "Authorization: Bearer $stok" "${BASE}/api/admin/partners" -o "$body"
  if has "$body" 'any(.[]; .slug == "acme")'; then
    ok "partner org 'acme' registered ($(jq -r 'map(select(.slug=="acme")) | .[0].managed_count' "$body") managed clients)"
  else
    bad "partner org 'acme' missing — partners-console-fleet has nothing to act as. Got: $(head -c 200 "$body")"
  fi

  curl -s --max-time 10 -H "Authorization: Bearer $stok" "${BASE}/api/customer-apps" -o "$body"
  if has "$body" 'any(.items[]; .slug == "oxy-starter" and .org_slug == "local")'; then
    ok "app registry lists oxy-starter under org 'local'"
  else
    bad "oxy-starter absent from the app registry — got: $(head -c 200 "$body")"
  fi
}

case "$MODE" in
  fleet) seed_fleet ;;
  native) seed_native ;;
  check) printf '\033[1m1. Seed: skipped (check-only)\033[0m\n' ;;
esac

verify

printf '\n\033[1m── fixtures: %d verified, %d missing ──\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
