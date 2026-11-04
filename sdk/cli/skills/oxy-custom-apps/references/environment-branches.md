# Environment branches: test any part of an app on its own copy

A non-production environment (`staging`, or a `dev-<handle>` sandbox) is a
**branch** of the app, the way a database branch is. It has:

- a **parent**: the environment it was created `--from`. Only the build comes
  from the parent. Data planes always fork from **production**, never from
  another sandbox;
- a **fork point** (`--at`): the moment production's data is copied from;
- a **mode per plane**: for each part of the app's state, how isolated this
  environment is.

The defaults keep a sandbox's old behaviour: reads hit production, and writes
are isolated or held. Turn on more isolation only for the parts you are testing.

Every command below takes the usual `--env <deployment>` (and `--token-env
OXY_TOKEN` for an agent; see [sandbox-agent-loop.md](sandbox-agent-loop.md)).

## Pick the modes by what you are testing

| You want to test | Use |
| --- | --- |
| A migration, or a function that edits records, against production's real rows | `--plane oltp=branch` (add `--at <ts>` for an exact moment) |
| A migration against production's structure with no rows | `--plane oltp=empty` |
| A function that appends Airhouse facts and reads them back | `--plane airhouse=branch` |
| A test that must see the same Airhouse rows every run | `--plane airhouse=snapshot` |
| What an email or notification would say, without sending it | `--plane messages=capture`, then `oxyc env outbox` |
| That the app works with none of staging's or production's secrets | `--plane secrets=empty`, then `env secret set` what it needs |
| That the app copes with no files yet | `--plane storage=empty` |
| A real write to a third-party **sandbox** account (Stripe test keys, a QuickBooks sandbox company, Square sandbox …) | `--fetch-host <host>` (repeatable; implies `fetch=allow`) |
| Writes to a warehouse database of the sandbox's own | `--destination <db>=<mapped>` |
| A semantic-model change on a git branch | `--plane semantic=branch:<branch>` |
| An automation the bundle starts, without its side effects | `--plane workspace-actions=preview` |
| As much isolation as has shipped, in one flag | `--preset full` |

Combine as many as you need. A choice you name always wins over the preset.

## The planes

| Plane (`key`) | Default | Other modes |
| --- | --- | --- |
| `build` | `own`: what you publish to it; `--from` starts it on the parent's build | — |
| `semantic` | `build-pin`: the model the build was published with | `branch:<b>`: the head of a workspace git branch, re-pinned on every push |
| `reads` (`ctx.query`, `ctx.warehouse.query`) | `live`: production, read-only | — (customer warehouses are never copied) |
| `oltp` (`ctx.oltp`) | `schema`: the app's schema in the org's shared staging branch | `branch`: its own database branch, production's tables **and rows** at the fork point · `empty`: its own branch, structure only |
| `airhouse` (`ctx.airhouse`) | `write-only`: writes go to a sibling schema, queries read production | `branch`: queries see the sandbox's own writes first, then production live · `snapshot`: the same, with the rest pinned to the fork point |
| `warehouse` (`ctx.warehouse` writes) | `mapped`: the manifest's `nonProduction.destinations` | per-environment overrides with `--destination` |
| `storage` (`ctx.storage`) | `branch`: its own files, and reads fall back to production's same key | `empty`: its own files only |
| `secrets` (`ctx.env`) | `inherit`: its own value, then staging's, then production's `shared` | `empty`: its own value only |
| `messages` (`ctx.email.send`, `ctx.notify`) | `invoker`: sent to the caller only, subject prefixed with the environment | `capture`: sent to nobody, kept in the outbox |
| `fetch` (mutating `ctx.fetch`) | `held-writes`: recorded, not sent | `allow` + a host list of registered provider sandboxes |
| `schedules` | `switch`: off until `oxyc env schedules <app> <env> --on` | — |
| `workspace-actions` (agent asks, automation runs) | `held` | `refused`: `403` · `preview`: an automation run executes as a held dry run |

`--preset full` sets `oltp=branch`, `secrets=empty`, `messages=capture`,
`airhouse=branch` and `workspace-actions=preview`. It never sets `fetch`
(opening third-party writes is a per-host decision) or `semantic` (a preset
cannot pick your branch). `storage` stays `branch`; add `--plane storage=empty`
too if you want it.

Fixed, whatever the modes:

- **Production has no planes.** No mode, preset or override writes to production.
- `ctx.airway.run` is refused in every non-production environment.
- `oltp=branch|empty` and `airhouse=branch|snapshot` are **sandbox-only**.
  Staging refuses them, so `--preset full` on staging is refused.
- `oltp=branch|empty` needs an org with an OLTP database. Without one, pass
  `--plane oltp=schema` with `--preset full`.

## The lifecycle

```bash
# Create: a branch of production's data, as of an hour ago, on staging's build
oxyc env create acme/store dev-x --from staging --at 2026-11-03T08:00:00Z --preset full --json
oxyc env show acme/store dev-x                     # the planes line: each mode, and where it came from
oxyc publish --app-env dev-x --json                # or skip it: --from already gave it staging's build

# Change one plane later (=default clears it; --replace sends exactly these flags)
oxyc env update acme/store dev-x --plane storage=empty
oxyc env update acme/store dev-x --plane fetch=allow --fetch-host connect.squareupsandbox.com

# Read what it did
oxyc env outbox acme/store dev-x --json            # what a capture environment kept
oxyc invocations held acme/store <invocation-id>   # what a call held, or sent under fetch=allow
oxyc env diff acme/store dev-x --json              # its OLTP schema against production's

# Start a plane over
oxyc env reset acme/store dev-x --plane storage,secrets,messages   # each answers done
oxyc env reset acme/store dev-x --plane oltp --at <ts>             # re-cuts: queued, poll env show
oxyc env reset acme/store dev-x --plane airhouse                   # drops its copies, re-pins to now

oxyc env delete acme/store dev-x --yes --wait      # tears its branches down
```

- **`--at`** is not in the future and is within the last 24 hours (the
  database's history window; Airhouse keeps snapshots one day too).
- **An OLTP branch is cut in the background.** `env show` reports
  `oltp_branch.status`: `cutting`, `migrating`, `ready`, `resetting` or `failed`.
  A reset that is queued but not started still reads `ready`, with
  `oltp_branch.recut_queued: true`, so wait for that to be `false` too.
  `ctx.oltp` is refused, with the reason, until it is `ready`. It is never
  routed to staging or production in the meantime, so poll before the first call.
- **Reset** puts a plane back to a clean copy of its source. A sandbox resets
  `oltp`, `airhouse`, `storage`, `secrets` and `messages`. Staging resets
  `messages` only, because the team shares the rest. A sandbox on
  `oltp=schema` has nothing to reset: publish again to re-seed it.
- **Expiry.** A sandbox is deleted after 7 idle days. One holding a copy of
  production's rows (`oltp=branch`, or `airhouse=branch|snapshot`) is also
  deleted 30 days after its fork point, however busy it is; `env show` gives
  the day as `copies_expire_at`. `env reset` of that plane starts the 30 days
  again, and so does turning such a plane on for a sandbox that held no copy.
  Moving `airhouse` back to `write-only` keeps the rows it copied, still on the
  same clock: `env reset --plane airhouse` drops them. `oltp=empty` holds no
  rows and is not deleted for age.
- **Entering `airhouse=snapshot`** on an existing sandbox pins the snapshot to
  that moment.
- **Limits.** 20 sandboxes per app. A per-org cap on OLTP branches (8 by
  default): delete sandboxes you are done with.

## The two workspace planes

- **`semantic=branch:<b>`**:
  - The branch must exist on the workspace's remote.
  - The write compiles the branch head and pins the environment to it, and a
    later push recompiles and moves the pin. Until a compile is ready, data
    requests answer `503 semantic_branch_not_ready` and functions are refused.
    Retry; never fall back to production's model. Repointing to another branch
    is `503` until the new one compiles.
  - If the deployment does not follow pushes, run the same command again to
    re-pin to the head.
  - `--plane semantic=default` returns to the build's own pin, and a build
    published with `--semantic-branch` wins over the plane.
  - `semantic=release:staging` does not exist yet. Don't promise it.
- **`workspace-actions=preview`**:
  - An automation run the bundle starts executes as a held dry run: writes are
    held, Airhouse writes go to an overlay shared by that environment's runs,
    and `http_request` is GET-only.
  - It answers `202 {run_id, mode: "preview"}`; poll it like any run.
  - An agent ask the bundle starts is still held.

## When it refuses

| Answer | Means | Do |
| --- | --- | --- |
| `400 invalid_planes` | the choice cannot be stored. The message says why: unknown plane or mode, a sandbox-only mode on staging, no OLTP database, the branch cap, `fetch=allow` with no host or an unregistered one, a semantic branch that does not exist, a warehouse override naming production | fix what it names |
| `400 invalid_parent` | `--from` names an environment you may not fork, or one that does not exist | fork from a sandbox you created (or staging, if your token allows it) |
| `409 plane_not_resettable` | nothing to reset there (`oltp=schema`, or a shared plane on staging) | publish again, or reset a sandbox instead |
| `409 environment_busy` | a teardown or migration holds it | retry |
| `409 oltp_branch_unready` | `env diff` before the branch is `ready`, or cut in the other mode | wait for `ready` |
| `503 semantic_branch_not_ready` (`Retry-After: 5`) | the branch is still compiling | retry. It is never answered from production's model |
| `409 preview_runs_disabled` | the deployment has preview runs off | report it. No live run happened |
| a host call inside the function answers `409` (held) | the write was recorded, not performed. The function itself still runs | read `oxyc invocations held`. The `note` names the mode or host that would let it through |

## A worked example: a payment flow, end to end

```bash
oxyc env create acme/store dev-pay --from staging --preset full --json
oxyc env update acme/store dev-pay --fetch-host connect.squareupsandbox.com
oxyc env secret set acme/store SQUARE_TOKEN --app-env dev-pay --value-env SQUARE_SANDBOX_TOKEN
oxyc fn call acme/store checkout --app-env dev-pay --data '{"cart":"c1"}' --json
oxyc invocations list acme/store --app-env dev-pay --function checkout --limit 1 --json
oxyc invocations held acme/store <id> --json   # fetch POST connect.squareupsandbox.com/v2/payments → 200 (sent)
oxyc env outbox acme/store dev-pay --json      # the receipt email it would have sent
oxyc env diff acme/store dev-pay --json        # what its migration changed, before promoting
oxyc env delete acme/store dev-pay --yes --wait
```

The order rows land in the sandbox's own copy of production's database, and
the Airhouse facts in its overlay. The receipt email is kept, not sent. The
Square call goes to Square's sandbox, using the sandbox's own token. Nothing
reaches production. To ship, a person runs `oxyc promote <app> --from dev-pay`
(the build becomes staging's), checks it there, then promotes to production.
