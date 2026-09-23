# Platform Canary — every host API live apps use, on a schedule

An Oxy-owned custom app. Its `canary` function runs every five minutes, and on
`oxyc checks run`. Each run exercises the host APIs that live custom apps
call, in an org no customer reads. Its page runs three browser checks and renders
the marker the release journey's browser check waits for.

It replaces `warehouse-canary`, which only covered `ctx.warehouse` writes.

## Why it exists

In 0.5.140–0.5.144 every `ctx.warehouse.insert` against ClickHouse failed with
`Code: 27`: the audit tag the host added was being read as a row. Reads kept
working. The first report came from a customer whose report upload broke, two
days in. No platform check wrote anything, so none of them looked wrong.

The same gap exists for every host API a live app calls and nothing on the
platform side calls: OLTP writes, org reads, binary storage, secret writes. The
canary makes each of those calls on a schedule, the way live apps make them.

## Scheduled steps

`functions/canary.ts` runs the steps in `functions/steps.ts` in this order and
stops at the first failure:

| Step | Exercises | Live dependents |
| --- | --- | --- |
| `warehouse_insert` | `ctx.warehouse.insert`, 3 rows in one statement | bookkeeping `ingest-report`, `ingest-doordash` |
| `warehouse_exec` | `ctx.warehouse.exec` of a 2-row INSERT that opens with `--` | warehouse `submit-receiving`; 14 functions call `exec` |
| `warehouse_readback` | `ctx.warehouse.query`: every row the two write steps sent landed | — |
| `upsert_refusal` | Pins a refusal: `ctx.warehouse.upsert` on ClickHouse must be refused by name (`warehouse.upsert is not supported on ClickHouse`) before anything is sent. Writes nothing | the docs' promise; any app that reaches for `upsert` on a warehouse |
| `tx_refusal` | Pins a refusal: `ctx.tx` on ClickHouse must be refused naming the engine (`ClickHouse does not support multi-statement transactions`), and the callback must not run | the docs' promise |
| `sql_read` | Top-level `ctx.query`, resolving `{ rows, truncated }` | 13 functions |
| `sql_stream` | `ctx.queryStream`, the same read through the async generator | — |
| `oltp_roundtrip` | `ctx.oltp` write, read and delete in the canary's own schema | Store Ops (45 functions) |
| `oltp_transaction` | `ctx.oltp.tx`: a transaction inserts a row and reads it back on its own connection, and the commit is visible afterwards; a second inserts and throws, and its row is gone. Deletes what it committed | — |
| `shape_zoo` | Every case in `fixtures/data-shapes/zoo.json` (synced to `functions/shape-zoo.json`). The ClickHouse cases are created, filled once and read one column at a time on `canary_warehouse` with `ctx.warehouse.exec`/`query`; the Postgres cases go the same way through `ctx.oltp`. Each column must equal its case's `expect` | every app that reads a warehouse or its OLTP store |
| `org_read` | `ctx.org.places()`, `people()` and `assignments()` response shapes | Store Ops (43) |
| `storage_roundtrip` | `put` binary as base64; `get` and compare bytes; `getUploadUrl`, then a `ctx.fetch` PUT with `bodyEncoding: "base64"`; `head`; `copy`, then `getDownloadUrl` and a `ctx.fetch` GET with `encoding: "base64"` compared to the bytes; `list` under `canary/`, walking pages, which must return all three; `delete` all three | bookkeeping, warehouse, Store Ops |
| `secrets_roundtrip` | `ctx.secrets.set`, read back through `ctx.env` | bookkeeping `refresh-qb-token` |
| `check_in` | `ctx.fetch` POST to the All Quiet check-in URL, after every other step passed | — |

A failing step throws `canary step <name> failed: <cause>`. The failure pager
fingerprints the start of that message, so each step pages under its own
fingerprint. URLs are stripped from the cause, because the check-in URL and a
presigned PUT are both credentials.

**Every host op has a step here, or a written reason.** `STEP_OPS` in
`functions/steps.ts` names the host ops each step makes; `pnpm test` checks each
list against the calls the step makes on the fake host, and
`crates/app/tests/custom_apps/canary_coverage.rs` fails when a name in the
platform's closed list (`HOST_OPS` in `custom_apps_functions/host_call_attrs.rs`)
has neither a step nor an exemption with a reason. Adding a host op means adding
a step, or an exemption that says what would unblock one.

The two `*_refusal` steps pin wording, not behaviour the canary wants: the host
refuses both ops on ClickHouse before sending anything, and the platform
classifies those refusals as the app's own condition (`bad_request`), so
catching them every five minutes pages nobody. A host that let either through
would fail in the engine's words, one row late.

**Publish a bundle with those two steps only to a server carrying that
classification.** The canary bundle and the server roll out independently, and
the host notes a paging host-call failure *before* the isolate sees the
rejection. On a server that predates `classify_host_error`'s `bad_request` for
these refusals (`custom_apps_functions/host_call_attrs.rs`), every run leaves
two `host_call_failed` fingerprints, `warehouse.upsert` and `tx.begin`, and
nothing recovers them: ops are paged twice every five minutes for the canary's
own contract check. Until that server release is deployed, keep `CANARY_STEPS`
explicit and without `upsert_refusal` and `tx_refusal`.

`shape_zoo` fails as `canary step shape_zoo failed: <key> (<class>): expected <json> got <json>`,
naming the first case that differs. Its values are synthetic. A failure there means an engine,
driver or conversion changed. CI's `custom_app_functions_shape_zoo` reads the same file, so check
there first whether a platform change caused it.

**`shape_zoo` runs by default, and it is the expensive step.** `CANARY_STEPS`
absent means all, and `shape_zoo` is in that list. One run issues roughly **200
statements** — a read per case, 115 ClickHouse and 81 Postgres, plus a create
and a count on each plane — all sequential. That is what `canary`'s
180-second `timeoutSeconds` is sized for; at the 60 seconds the other steps
need, the step times out rather than failing on a case. The steps added after
the zoo was timed — `upsert_refusal`, `tx_refusal`, `sql_stream`,
`oltp_transaction`, the three `org_read` reads and the copy, presigned GET and
list in `storage_roundtrip` — add about fifteen host calls, a few seconds at
most, inside the same budget. A deployment that has
not timed the zoo against its own warehouse should list `CANARY_STEPS` **without**
`shape_zoo` until it has, and note why next to it. Timing it needs no stopwatch:
a passing step logs one line, `shape_zoo: <cases> cases in <ms> ms` (no values),
in the invocation's log. The line is `info`, so it is **not** in the pod logs —
the platform filter holds an app's `ctx.log()` lines at `warn`. Read it from
`oxyc api "/api/customer-apps/$ORG/$APP/logs"` (app-admin standing) or from the
job's `/function-runs/<run_id>`.

### Tags and cleanup

- **Tags:** every row, object and secret value carries the run id.
- **Removed in the same run:** the OLTP rows (`oltp_roundtrip`'s, and
  `oltp_transaction`'s `<runId>-tx`) and all three storage objects. If that
  cleanup fails, the step fails.
- **ClickHouse rows** are not deleted. The table's 7-day TTL removes them.
  `upsert_refusal` and `tx_refusal` write none.
- **Zoo tables** (`oxy_shape_zoo_<first 8 hex chars of the zoo's SHA-256>`, in `oxy_canary` and
  in the canary's OLTP schema) are not deleted: each holds one row, and a run creates and fills it
  only when it is missing. A zoo change creates a new table; drop the old one by hand.

**`secrets_roundtrip` spans two runs.** `ctx.env` is resolved when a run
starts, so a run can't read back a value it just set. Each run instead:

1. writes `CANARY_SECRET_ROUNDTRIP`;
2. checks the value the previous run wrote.

The step fails in two cases:

- **No value yet.** This happens once, on the very first run.
- **The value is over an hour old.** That's twelve scheduled runs: either
  writes aren't reaching `ctx.env`, or the canary stopped running.

**So on a fresh deploy, or after a schedule gap over an hour, `secrets_roundtrip`
fails once:** it reads the value the previous run wrote. A gate run on a fresh
stack should run the canary twice, or drop the step with `CANARY_STEPS`.

### Not covered, and why

Each of these is an `EXEMPT` entry in `canary_coverage.rs`, with the reason and
what would unblock it; that test fails if one is dropped from the platform's op
list or gains a step without the entry going.

- `email.send`: a real SES send every five minutes is a side effect outside the
  platform, to a mailbox someone has to own — open decision D3 in the
  verification design (`internal-docs/2026-09-14-custom-app-verification-design.md`).
- `semantic.query`: the canary workspace has no semantic model, so a call would
  fail on the missing topic rather than prove the host path. Unblocked by
  committing a minimal view and topic to the workspace.
- `airway.run`: runs an ELT pipeline, a side effect that needs an `.airway.yml`
  and source credentials. Out of scope for the canary by design.
- `airhouse.query` / `exec` / `append`: `ctx.airhouse` writes the app's own
  schema in the workspace's Airhouse, and its tables come only from
  `airhouseMigrations` applied at promote. The manifest declares neither, and
  neither staging nor prod is known to provision Airhouse for `oxy-canary`.
  Unblocked by provisioning it, declaring the capability and a one-table
  migration, and a step that appends one run-tagged row and reads it back.

`ctx.tx` and `warehouse.upsert` are covered only as refusals (`tx_refusal`,
`upsert_refusal`): the canary's destination is ClickHouse, which can do
neither. `ctx.oltp.tx` exercises the transaction bracket itself on Postgres.

## Page checks

`src/checks.ts` runs these in order once the page mounts:

| Check | Exercises |
| --- | --- |
| `echo` | `useFunction("echo")` sends a nonce and gets it back: session cookie → function → SSE result |
| `sql_query_route` | `POST /api/{projectId}/sql/query` with `SELECT 1 AS one` against `canary_warehouse`, sent and parsed the way bookkeeping's `useSqlQuery` hook does |
| `sdk_query` | The SDK's `useQuery`, same SQL and database |

The page renders the result on `<main>`:

- **All passed:** `data-canary="ready"`.
- **A check failed:** `data-canary-failed="<check-name>"`.
- **The manifest didn't load**, so no check ran: `data-canary-failed="manifest"`.

## Running the checks

```sh
oxyc checks run oxy-canary/platform-canary
```

This runs every function marked `"check": true` in `oxy-app.json` (here, `canary`).
A check fails when it throws, times out, or returns a body whose `ok` is
`false`. `canary` never returns `ok: false`: every failure throws.

## Running it in CI and locally

Checkpoint 1 of the verification design
(`internal-docs/2026-09-14-custom-app-verification-design.md` §3) runs this app on
every pull request that touches the custom-app surface, the engines, `sdk/`, this
directory or the zoo fixture: the `custom-app-canary` job in
`.github/workflows/ci.yaml`. It boots the PR's own server from the binary CI just
built, builds `@oxy-hq/sdk`, `@oxy-hq/vite-plugin` and `oxyc` from the checkout,
publishes this app with them, runs `oxyc checks run` twice and opens the app in
Chromium. The script is `scripts/ci/platform-canary-checkpoint.mjs`, and a failure
names the step.

- **Built against the checkout, not npm.** The script publishes a staged copy of
  this directory whose `pnpm-workspace.yaml` overrides `@oxy-hq/sdk` and
  `@oxy-hq/vite-plugin` with `pnpm pack` tarballs of the workspace packages. This
  directory's `package.json` and lockfile stay as they are, so a plain
  `pnpm install` here still takes the npm versions.
- **Steps CI runs are derived:** everything in `ALL_STEPS` (`functions/steps.ts`)
  minus the script's `OMITTED_IN_CI`, each omission with its reason: `check_in`
  (no All Quiet monitor to check in to) and `storage_roundtrip` (its presigned
  PUT goes through `ctx.fetch`, which allows only HTTPS to a public host, and
  the runner's object store would be MinIO on loopback). A step added here runs
  in CI the day it lands; `node scripts/ci/platform-canary-checkpoint.mjs
  --list-steps` prints the list, and the script's self-test fails when an
  omission names no real step. `oltp_roundtrip`, `oltp_transaction` and the
  Postgres half of `shape_zoo` run: the server's local OLTP provider gives the
  org a database.
- **Two runs.** The first may fail only `secrets_roundtrip` (see above: it reads
  the value the previous run wrote); the second must be green.
- **What it creates**, all through the API: an org (default `oxy-canary`) whose
  Owner is `canary@oxygen-hq.com`, minted by dev sign-in — Owner rather than the
  Member prod uses, because minting the API key needs workspace Admin; the
  `app_operator` grant scoped to that org, as on prod; the `oltp` flag on and a
  writer provisioned; the ClickHouse database and the `canary_warehouse`
  workspace database; a compiled, promoted revision; an API key that expires in
  a day. Re-runs reuse the org and leave its database configuration alone, so
  point a fresh `--org-slug` at a different ClickHouse database.

**Locally**, against a `just up` box: add
`OXY_DEV_LOGIN_EMAILS=<first OXY_GLOBAL_ADMINS entry>,canary@oxygen-hq.com` to
`.env` (dev sign-in must know the canary user; this replaces the persona roster for
that box, so remove it after), `just up --restart`, then

```sh
just custom-app-canary            # against :3000 and the oxy-clickhouse container
just custom-app-canary --journey  # plus the browser check (Playwright's Chromium in web-app)
```

Against any other server: `node scripts/ci/platform-canary-checkpoint.mjs --target
<url> --clickhouse-url <url the server can reach> --clickhouse-password <pw>`;
`--help` lists the rest. The server needs `OXY_OLTP_PROVIDER=local` with
`OXY_OLTP_ADMIN_URL`, and dev sign-in for a staff address and the canary user.

## Configuration

Both are app secrets (the app's Secrets panel):

| Secret | Meaning |
| --- | --- |
| `CANARY_STEPS` | Comma list of steps to run. Absent means all. An unknown name fails the run. |
| `CANARY_CHECKIN_URL` | The All Quiet cron check-in URL. Needed while `check_in` runs: the step fails closed without it. Not declared `required` in the manifest. Prod only. |

`CANARY_SECRET_ROUNDTRIP` also appears on the panel. The canary writes it; never
set it by hand.

`CANARY_STEPS` is the only way to drop a step, so an omission is always a
recorded decision. Note why next to it. For example, the `oltp` feature flag is a
global kill switch: where it's off and the list still names `oltp_roundtrip`,
that step fails.

### Per environment

- **Prod** runs every step, `check_in` included. Leave `CANARY_STEPS` unset
  and set `CANARY_CHECKIN_URL`. The URL exists only once the All Quiet
  monitors are applied: it is `platform_canary_prod` in
  `terraform output -json cron_checkin_urls` (setup step 2 in
  [release-process.md § Release checks](../../../internal-docs/release-process.md#release-checks)).
  Until it's set, every prod run fails `check_in`, and three failed runs page.
- **Staging** has no check-in monitor yet. It gets one when Better Stack's
  escalation flag flips (design doc §5.2). Until then, staging sets
  `CANARY_STEPS` to every step except `check_in` and leaves
  `CANARY_CHECKIN_URL` unset:

  ```text
  warehouse_insert,warehouse_exec,warehouse_readback,upsert_refusal,tx_refusal,sql_read,sql_stream,oltp_roundtrip,oltp_transaction,shape_zoo,org_read,storage_roundtrip,secrets_roundtrip
  ```

  The reason to note next to it: no staging monitor. Three things follow:
  - A step added to `ALL_STEPS` does not run on staging until this list names
    it: `CANARY_STEPS` names steps, and a name it lacks is a recorded
    omission. The list above is current as of `oltp_transaction`; update the
    secret when a step lands — and add `upsert_refusal` and `tx_refusal` only
    once the staging server carries the refusal classification (the paragraph
    under the step table), or every run pages twice.
  - The manifest does not declare `CANARY_CHECKIN_URL` required, so staging's
    Secrets panel does not list it as missing: an unset URL is staging's normal
    state, not a gap. The panel's "missing" flag is for secrets a run cannot
    do without.
  - `check_in` still fails closed. Put it back in the list without the URL
    and every run fails with `CANARY_CHECKIN_URL is not set`.

  **`shape_zoo` stays in that list on purpose.** Staging and prod are the two
  deployments the step is meant to run on, and staging sees a zoo change first.
  The "drop it until you have timed it" advice above is for any *other*
  deployment, not for these two.

## How a failure reaches anyone

A failed run is an ordinary failed invocation:

- It logs one `WARN` line on target `oxy::app_function` to the platform log
  (HyperDX), with `error.type` and `error.fingerprint`.
- It marks the invocation span `ERROR`.
- Three failures carrying a fingerprint the function hasn't had in a week page
  `OXY_OPS_SLACK_CHANNEL` (`custom_apps_functions::failure_alert`).

`check_in` runs only after every other step passed. So on prod, All Quiet
alerts when the canary stops passing *or* stops running: a stopped schedule or a
dead worker fails no invocation, and only a missed check-in notices. The monitor
is `Warning`, so it doesn't page. It becomes `Critical` after a 7-day green
streak and a fire drill. Staging has no monitor (see
[Per environment](#per-environment)).

At a five-minute cadence, a deploy that breaks one of these APIs pages about
fifteen minutes after it lands, whether or not any customer is using it.

The page rule is "new in a week", so a canary failure can go unpaged:

- **Never paged:** the same failure returns within a week of its last page,
  for example a bad change that was rolled back and then redeployed.
- **Paged late:** the canary already paged for another failure in the last six
  hours. The new failure is held back until that window passes, then pages on
  its next run.

Both cases still log the `WARN` line and mark the span. After any
rollback-and-redeploy, check the canary's invocations in the admin console.

## Per-deployment setup

Once per deployment (dev, staging, prod), by a human:

- [ ] **Org and workspace.** Staff creates org `oxy-canary` with workspace
      `canary`.
  - Never use `poke-house-staging`: despite the name it runs on prod and writes
    Poke House's tables.
  - Never use a customer org.
- [ ] **User.** Create `canary@oxygen-hq.com` as a member of `oxy-canary` only.
  - **Inbox:** the address needs one, for the single interactive sign-in that
    mints its key.
  - **API key:** give it a 90-day `expires_at`, and store it as the GitHub
    Actions environment secret. Keys aren't scoped to an app or project, so
    single-org membership is the containment. An expired key fails the release
    journey loudly, so rotate before it lapses.
  - **Grant:** `app_operator`, scoped to `oxy-canary`.
- [ ] **ClickHouse.** Create database `oxy_canary` and user `oxy_canary`, with
      `CREATE TABLE`, `INSERT` and `SELECT` on that database and nothing else.
      Where it lives:
  - **prod:** the oxy-observability CHI.
  - **staging:** oxy-dev's observability CHI.
  - `shape_zoo` creates `JSON`, `Tuple`, `Map`, `AggregateFunction` and wide-integer columns.
    Before enabling it, check `SELECT version()` on that server: `JSON` needs 25.3 or later.
- [ ] **`canary_warehouse` in the workspace `config.yml`,** under the name the
      manifest allows (`destinations: ["canary_warehouse"]`):

  ```yaml
  databases:
    - name: canary_warehouse
      type: clickhouse
      host: https://<clickhouse-host>:8443
      user: oxy_canary
      password_var: CANARY_CLICKHOUSE_PASSWORD
      database: oxy_canary
  ```

  - Store `CANARY_CLICKHOUSE_PASSWORD` in the workspace's secrets.
  - Keep `canary_warehouse` the workspace's default database. `sql_read` calls
    `ctx.query`, which only reaches the default.
- [ ] **OLTP provision.** The writer name is the slug with hyphens as
      underscores. `--org` takes the org's UUID or the email of a user in exactly
      one org, not a slug. This uses the canary user, who belongs only to
      `oxy-canary`; the org's UUID works too:

  ```sh
  oxy oltp provision --org canary@oxygen-hq.com --writer app:platform_canary
  ```

  That mints the `app_platform_canary_rw` role and its schema. The `oltp`
  feature flag must be on.
- [ ] **A place.** Add one location under Settings → Organization. `org_read`
      passes on an empty registry, but then it has no place whose shape to check.
- [ ] **Storage.** The deployment needs `OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET`.
      Without a bucket, the presigned URL from `getUploadUrl` may not be one
      `ctx.fetch` can reach (HTTPS only, no private hosts), and
      `storage_roundtrip` fails.
- [ ] **Secrets.** On the app's Secrets panel, per
      [Per environment](#per-environment):
  - **Prod:** set `CANARY_CHECKIN_URL` to `platform_canary_prod`'s URL from
    `terraform output -json cron_checkin_urls`, an All Quiet cron monitor
    expecting a check-in every five minutes. Apply the monitors first, at
    `Warning` (see [Per environment](#per-environment)). Leave `CANARY_STEPS`
    unset.
  - **Staging:** set `CANARY_STEPS` to the list without `check_in`. Leave
    `CANARY_CHECKIN_URL` unset.
  - **Anywhere else a step must be dropped:** set `CANARY_STEPS` and note why.
- [ ] **Global worker.** Scheduled runs need `OXY_INPROC_GLOBAL_WORKER`. With it
      off, the schedule exists and never fires. On prod, the missed check-in is
      the only signal.
- [ ] **`oxyc publish`** from this directory, after `pnpm install`. Publishing
      registers the `*/5 * * * *` schedule.
- [ ] **Verify.** Run `oxyc checks run oxy-canary/platform-canary` twice:
  - The first run fails `secrets_roundtrip`, because no earlier run wrote the
    value.
  - The second run passes. On prod it also checks in.
  - Then open the app and confirm `<main data-canary="ready">`.

## Developing

```sh
pnpm install     # this directory is its own pnpm root (pnpm-workspace.yaml)
pnpm test        # steps against a fake ctx, the bundle loading, the page checks and marker
pnpm typecheck
pnpm build
```

**After editing `fixtures/data-shapes/zoo.json`,** run `node scripts/data-shapes/sync-canary-zoo.mjs`
from the repo root. `pnpm test` fails until the copy and its digest match the fixture.

Install with plain `pnpm install` in this directory: `--ignore-workspace` would
skip this directory's `pnpm-workspace.yaml` and its esbuild build decision, and
the install would exit 1.

**Functions import only types from `@oxy-hq/sdk`.** Its root entry also loads
React, whose CommonJS entry reads `process.env` at load. The isolate has no
`process`, so any value import fails every run before a step starts. The bundle
test in `functions/steps.test.ts` catches that.
