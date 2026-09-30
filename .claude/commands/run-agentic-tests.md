---
name: run-agentic-tests
description: Run agentic browser tests with HEADED=1 DEBUG=1 — the runner auto-spawns the right backend based on each flow's backend_mode setting
activeForm: Running agentic browser tests
argument-hint: "[flow1 flow2 ...]"
allowed-tools:
  - Bash
  - Read
---

# Run agentic browser tests

Wraps `pnpm test:agentic` with `HEADED=1 DEBUG=1` so the dev sees the
browser and the per-iteration LLM reasoning.

**The runner auto-spawns the right oxy backend** based on each flow's
`settings.backend_mode` (`cloud`, the default, → enterprise mode: `oxy start
--enterprise`, `oxy seed` of `demo_project/`, and a dev-login session as
`flow@oxy.local`, all on port 3000; `local` → the legacy `oxy start --local
--enterprise`). No port-probing or pre-start dance needed.

`$ARGUMENTS` is one or more **positional flow-name substring filters**
(OR-combined). A flow matches if its filename contains ANY of the listed
substrings. Empty `$ARGUMENTS` = all flows.

This command must be run from the root of an `oxy-hq/oxygen-internal`
checkout.

## Steps

### 1. Sanity-check the working directory

```bash
test -f web-app/tests/agentic/README.md && test -f json-schemas/flow-test.json
```

If either path is missing, exit and tell the dev: "This command must be
run from the root of an oxy-hq/oxygen-internal checkout where the agentic
browser-test layer is present (see `web-app/tests/agentic/README.md`)."

### 2. Confirm `ANTHROPIC_API_KEY` is set

The runner exits 2 with `ERROR: ANTHROPIC_API_KEY is required` if it's
unset (see `runner/cli.ts`). Fail loudly before spawning anything:

```bash
if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
  echo "Set ANTHROPIC_API_KEY first: export it, or add to web-app/.env.local."
  exit 2
fi
```

### 3. Validate Docker is up (the runner needs it for Postgres)

```bash
docker info >/dev/null 2>&1 || {
  echo "Docker Desktop must be running — \`oxy start\` brings up Postgres in a container."
  exit 2
}
```

### 4. Run the tests

From `web-app/`:

```bash
cd web-app
HEADED=1 DEBUG=1 pnpm test:agentic $ARGUMENTS
```

The runner reads each loaded flow's `settings.backend_mode` (default
`cloud`, enterprise mode), spawns the right `oxy start …` invocation and
seeds `demo_project/` itself. If a backend is already healthy at the
resolved URL, the runner uses it as-is: no respawn and **no seed** — a
`just up` stack has `examples/` as its Demo workspace and does not allow
`flow@oxy.local` to sign in (see the runner README, "Run").

**The runner errors loudly if a single invocation mixes `backend_mode`
across flows** — filter to one mode at a time (typically by passing
flow-name substrings from the same bucket).

### 5. Report

After the run completes (or fails), surface:

1. **Markdown report path**:
   ```
   web-app/tests/agentic/.results/<ts>.md
   ```
2. **JSON next to it** (same data programmatically).
3. **Trace on failure**:
   ```
   web-app/tests/agentic/.traces/<flow>-<case>.zip
   pnpm exec playwright show-trace web-app/tests/agentic/.traces/<flow>-<case>.zip
   ```
4. **Grand cost** printed at the top of the run output (per step, per
   run, per total).
5. **Cost-budget overage warnings** — the reporter compares observed cost
   against `web-app/tests/agentic/flows/_budgets.yml` ceilings and writes
   `⚠️` to the markdown summary on overage. Advisory only.

If healing happened (`.results/healing.json` non-empty), surface the
`pnpm test:agentic --accept-healing <flow>` command and route the dev to
`/fix-agentic-test` for full triage.

## Escape hatch — driving your own `oxy serve`

When the dev wants to drive a backend they started themselves (e.g. to
debug with a persistent Postgres volume across runs, attach a debugger,
run a cloud-mode flow that needs an org — no UI creates one, so it must
be `oxy seed`ed first), pass both `--no-auto-backend` and
`--no-auto-frontend` and set `OXY_BASE_URL` / `OXY_HEALTH_URL`:

```bash
# Terminal 1 — start oxy yourself (cloud mode in this example)
oxy-debug start --enterprise            # persistent Postgres state
# once it is healthy: the `local` org + a compiled Demo workspace
OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy \
  oxy-debug seed --workspace-path ./examples

# Terminal 2 — point the runner at it, signed in as a staff identity the
# server's dev-login allows (the admin/airway flows need staff)
OXY_FLOW_EMAIL=<staff email from OXY_GLOBAL_ADMINS> \
  pnpm test:agentic airway-pipeline-run.flow.test.yml --no-auto-backend --no-auto-frontend
```

This is documented as an escape hatch only — the default
auto-spawn path is faster for routine iteration.

## CI fast-path — `agentic_only` dispatch

For the "I just iterated on a flow YAML, what's the fast CI loop?" case,
mention the `agentic_only` workflow_dispatch input. It cuts CI feedback
from ~45 min to ~15 min by skipping typos / fmt-web / build-web / smoke /
E2E / cargo clippy / cargo nextest — only the changesets gate + cargo
build + the agentic matrix run:

```bash
gh workflow run "CI check" --repo oxy-hq/oxygen-internal \
  --ref <branch> --field agentic_only=true
```

## Error handling

- **`agentic runner: cannot run flows with mixed backend_mode`** — the
  positional filters matched a flow opted into legacy `local` mode
  alongside enterprise ones. Filter to one mode at a time.
- **`[session] dev-login as flow@oxy.local failed`** — you are reusing a
  backend that does not allow the identity (e.g. `just up`). Stop it, or
  set `OXY_FLOW_EMAIL` to an identity it allows.
- **`backend did not become healthy`** / **`oxy seed failed`** — `oxy start
  --enterprise` or the demo_project seed failed. Tail
  `web-app/tests/agentic/.logs/backend.log`. Common causes: Docker
  Desktop not running, system `oxy` on PATH older than the workspace
  build (set `$OXY_BIN=$PWD/target/debug/oxy`).
- **`--enterprise: unrecognized argument`** — the `oxy` binary on PATH is
  older than the workspace build. `export OXY_BIN=$PWD/target/debug/oxy`
  and re-run.
- **Tier-2 healing happened** — the run posted a healing-staging entry.
  Route to `/fix-agentic-test <flow>` for triage.

## Notes

- Never strip `HEADED=1 DEBUG=1` from the wrapped command — those are the
  defaults that make this useful for interactive runs. Devs who want a
  quiet run can invoke `pnpm test:agentic` directly.
- Don't pre-probe ports or pre-start the backend. The runner does this
  itself based on `backend_mode`. The legacy probe-then-spawn flow was
  dead weight after the runner gained auto-spawn.
- Multi-positional filters are OR-combined. `pnpm test:agentic chat ide`
  runs every flow whose filename contains either substring.
