# `@oxy-hq/cli`

`oxyc` — a `gh api`-shaped client for the Oxy HTTP API, plus the tooling that
manages customer workspace repos.

- **API client** — `api`, `routes`, `schema`, `openapi`, `login`, `whoami`, `assume`, `oltp`
- **Customer workspaces** — `list`, `new`, `import`, `doctor`, `update`, `adopt`, `launch`
- **Custom apps** — `publish`, `init-ci`, `proxy`, `apps`
- **Checks** — `checks run`
- **Sandboxes** — `env`, `fn call`, `invocations`, `logs`
- **Workspace previews** — `preview`
- **Development** — `validate`, `mcp`, `guide`, `skills`

## Install

```bash
npm install -g @oxy-hq/cli      # then: oxyc
npx @oxy-hq/cli routes          # zero-install; works for everything but `skills install`
```

From a checkout:

```bash
cd sdk/cli && pnpm install && pnpm build
node dist/main.mjs --help
pnpm link --global              # or, for `oxyc` on PATH
```

Requires `node >= 20`. `gh` (authenticated) is required for the customer
commands, `jq` for `--jq`, `git` for the repo commands. Each is reported by
name with its install line the first time a command needs it.

## Quick start

```bash
oxyc login --env dev                        # browser flow; stores a token
oxyc whoami --env dev                       # verify it against the deployment

oxyc routes threads                         # find the endpoint
oxyc schema {workspace}/threads -X POST     # find the body it takes
oxyc api {workspace}/threads --md           # call it
```

## Global flags

Accepted by every command except `validate`, `guide`, `exit-codes`, `skills`
and `cache`.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--env <name\|url>` | `production` | deployment to target |
| `--target <url>` | — | explicit base URL; overrides `--env` |
| `--token-env <VAR>` | `OXY_TOKEN` | env var holding the bearer token |
| `--api-key-env <VAR>` | `OXY_API_KEY` | env var holding the key for `/external/api` |
| `--org <slug>` | — | value for the `{org}` placeholder |
| `--workspace <id>` | — | value for the `{workspace}` placeholder |
| `--project <id>` | — | value for the `{project}` placeholder |
| `--customer <name>` | — | act as though run inside this customer's repo |
| `--quiet` | — | suppress progress messages on stderr |

### Environments

`--env` takes a name or any URL. Anything not in this table is used as a URL.

| `--env` | URL |
| --- | --- |
| `local` | `http://localhost:5173` |
| `dev`, `development` | `https://aip.dev.oxy.tech` |
| `staging` | `https://aip.staging.oxy.tech` |
| `production`, `prod` (default) | `https://app.oxygen-hq.com` |

Credentials are stored **per deployment**, and the default is production. A
401 against dev usually means you are logged into prod — pass `--env` to
`login` and to the call.

## `oxyc api`

```
oxyc api <path> [flags]
```

`<path>` is relative to `/api`; a leading `/` or `api/` is accepted. One
exception: `customer-apps/…` is sent as written, because `/customer-apps/<org>/<app>/…`
is where an app's bundle is served. The app registry API is `api/customer-apps/…`
— spell the `api/` out for it (a 404 on the bare form says so).

| Flag | Meaning |
| --- | --- |
| `-X, --method <verb>` | HTTP method. Default GET, or POST when a body is present. |
| `-f, --raw-field <k=v>` | string parameter |
| `-F, --field <k=v>` | typed parameter — `true`, `3`, `["a"]`, `@file`, `@-` |
| `-H, --header <name:value>` | extra header (repeatable) |
| `--input <file\|->` | raw request body from a file, or `-` for stdin |
| `-q, --jq <expr>` | filter the response through `jq` |
| `--md` | render the result as a markdown table |
| `--paginate` | follow every page and return one document |
| `--paginate-key <field>` | the field holding the rows, when the guess is wrong |
| `--max-pages <n>` | stop after `n` pages (default 100) |
| `--slurp` | with `--paginate`, emit an array of pages instead of merging |
| `--cache <duration>` | reuse a recent successful GET (`30s`, `5m`, `2h`) |
| `-i, --include` | print the status line and response headers |
| `--silent` | make the request, print nothing |
| `--verbose` | log the request before making it |
| `--timeout <duration>` | request timeout (default `2m`) |

**Bodies.** `-F 'ids[]=a' -F 'ids[]=b'` accumulates into an array. On GET, HEAD
and DELETE, fields become query parameters instead of a body.

**Placeholders.** `{org}`, `{workspace}`, `{project}`, `{customer}` and `{me}`
are resolved from the customer repo you are standing in, from an org URL passed
to `--env`, or from `--org` / `--workspace` / `--project`. An unresolvable
placeholder is an error naming the flag that fills it — never a literal sent to
the server.

**Surfaces.** The path selects the credential: `/api/**` sends the bearer from
`oxyc login`; `/external/api/**` sends `X-API-Key` from `$OXY_API_KEY`.

**`--paginate` is a heuristic.** Oxy sends `Link: rel="next"` on some endpoints
and nothing on others; where there is no header, `oxyc` reads `next_offset`
(and asks again with `?offset=`), then `pagination.has_next`, then `has_more`,
then `page < total_pages` (asking again with `?page=`). Nothing recognised
means one page, and it warns when the server said nothing at all.

```bash
oxyc api {org}/workspaces --md
oxyc api {workspace}/threads -q '.threads[].title'
oxyc api {workspace}/sql/query -X POST -f 'sql=select 1' --md
oxyc api /admin/users --paginate --md
```

## Discovery

```bash
oxyc routes [filter] [--json] [--all] [--refresh]   # endpoints this deployment mounts
oxyc schema <path> [-X <verb>]                      # request/response shape for one
oxyc openapi                                        # the whole OpenAPI document
```

`routes` is served by `GET /api/_catalog`, so it describes the deployment you
are talking to rather than a baked table. It is cached per host; `--refresh`
re-asks. `--all` includes ide-only and worker-only mounts. `--json` adds each
route's surface and fleet role.

## Authentication

```bash
oxyc login [--env <e>] [--login-env <e...>] [--assume <org> -r <why>]
oxyc whoami [--json]
oxyc token                  # print the bearer, for a raw curl
oxyc logout
```

`--login-env` is repeatable and comma-separated (`--login-env dev,staging`);
the browser opens once per environment, in sequence.

Credentials live in the OS config directory under **`oxy`** — the same file the
Rust `oxy login` wrote before it was removed, so an existing login still works:

- macOS: `~/Library/Application Support/oxy/credentials.json`
- Linux: `$XDG_CONFIG_HOME/oxy/credentials.json`

`OXY_CREDENTIALS_PATH` overrides the path. Caches are separate, under `oxyc`
(`~/.cache/oxyc`). In CI set `OXY_TOKEN` instead of logging in.

## Acting as an organization

```bash
oxyc assume start --org <slug|uuid|url> -r "<reason>"
oxyc assume status [--json]
oxyc assume end [--org <slug>] [--all]
```

Sessions last **60 minutes and are not renewable** — re-running `start` returns
the existing session rather than extending it. `--org` is optional when `--env`
is itself an org URL. The reason is recorded in the audit log.

The session belongs to your account, not this terminal, so your browser shares
it and `end` ends it in both. A staff 403 on a tenant surface is usually no
active session rather than a role problem; `assume status` answers "none"
without failing.

## OLTP databases

Staff can see and request an org's transactional Postgres — the store behind
`ctx.oltp` — without a shell on the server.

```bash
oxyc oltp status [--org <slug|uuid|url>] [--json]
oxyc oltp provision --org <slug|uuid|url> --writer app:<slug> [--writer pipeline:<source> …] [--yes] [--json]
oxyc oltp provision --org <slug|uuid|url> --branch staging [--yes] [--json]
oxyc oltp reset --org <slug|uuid|url> --branch staging [--yes] [--json]
```

`status` lists every org, with a database or without; with `--org` it shows
that org's store and writers. No credential is ever printed.

`provision` prints what it will create and asks first, because it calls a paid
provider; without a terminal it refuses unless `--yes`. It is idempotent — an
existing database or writer is reconciled, not duplicated. `app:<slug>` is
derived the way the platform derives `ctx.oltp`'s schema (`app:store-ops` →
`app:store_ops`, schema `app_store_ops`); `pipeline:<source>` names
`raw_<source>`. A 404 can be an org outside your staff grant's scope, and an
active `assume` session closes `/admin`.

`--branch staging` also cuts the org's **staging branch**: one copy of the whole
database per org, which every app's staging environment writes to. It is made
only by this flag — never by a publish — and re-running it mints whatever a
writer added since the cut needs, without resetting anyone's data. `status`
shows its age and calls it stale past 30 days; nothing resets it on a timer.
`reset --branch staging` re-copies it from production, discarding every app's
staging data in the org, so it lists those apps and asks first (`--yes`
without a terminal).

## Customer workspaces

A customer **is** a GitHub repo carrying the `oxy-customer` topic. Tagging the
repo is the whole of registration; there is no separate registry file. These
commands need `gh` authenticated.

```bash
oxyc list [--json] [--refresh]        # the customers
oxyc path <customer>                  # where their repo is on this machine
oxyc new <customer> [--display <name>]        # create, tag and scaffold a repo
oxyc import <org/repo> [--clone]              # tag an existing repo
oxyc remove|rm <customer> [--purge --yes]     # untag (never deletes the repo)
oxyc doctor [<customer>] [--all]              # report state, change nothing
oxyc update <customer> [--apply] [--diff-all] # drift from the workspace template
oxyc adopt <customer> [--apply]               # install managed files an import lacks
oxyc activity <customer> [--since <date>] [--repo <org/name>] [--write] [--json]
oxyc launch <customer> [claude-args...] [--here] [--dry-run]
oxyc repos [--refresh]                # where OUR repos are checked out
```

`update` and `adopt` **report by default and write only with `--apply`**.
Neither commits, branches or pushes. What they may rewrite is decided by
`template/.oxyc-managed`: an unclassified file belongs to the customer and is
never touched.

`launch` starts a Claude Code session scoped to one customer; `--here` runs in
the current directory while granting access to the customer's repo.

## Custom apps

```bash
oxyc publish [--env <e>] [--dir <path>] [--promote] [--build-only | --prebuilt] [--json] [--allow-function-lint] [--app-env <dev-handle>]
oxyc init-ci [--app <org>/<app>] [--environment <name>] [--force]
oxyc proxy [--port <n>] [--allow-writes] [--allow-events] [--yes]
```

**`publish`**, from an app directory, runs `oxy-app.json`'s build (default
`pnpm install`, `pnpm build`, output `out/`), bundles each declared Oxy Function
with `pnpm exec esbuild` into `functions/<name>.js`, resolves the project from
the target's public `build-config`, and uploads the bundle — to the **draft**
channel unless `--promote`. `--env` defaults to **production**; name it.

- **Identity**: `--org` / `--app`, then `OXY_ORG` / `OXY_APP`, then the manifest's
  `orgSlug` / `slug`, then an `apps/<org>/<app>/` working directory. `--org`
  takes a slug or a UUID. `--project` pins the workspace (and implies its org).
- **`--dir`** publishes a pre-built directory instead of building; functions are
  still bundled into it. **`--build-only`** stops after building and bundling —
  no credential, no network unless the org must come from `--project`.
  **`--prebuilt`** (with `--dir`) skips esbuild and refuses if a declared
  function's `functions/<name>.js` is missing. The pair splits CI so the job that
  runs package scripts never holds the credential.
- **Auth**: the `--token-env` variable (`OXY_TOKEN`), then the login cache — or,
  in a GitHub Actions job with `id-token: write` and neither set, **trusted
  publishing**: the job's OIDC token is exchanged for a credential scoped to this
  one app. That needs the org **slug** and a publisher registered for the
  workflow (see `init-ci`).
- **Provenance**: the checkout's `origin`, `HEAD` and branch (else `GITHUB_SHA` /
  `GITHUB_REF_NAME`) are recorded; a missing half or a dirty app directory is a
  warning, never a failure. The build id defaults to
  `$GITHUB_SHA-$GITHUB_RUN_ID.$GITHUB_RUN_ATTEMPT`, else random — a reused one is
  a `409`.
- `.env.local`, then `.env`, are read from the directory and its parents without
  overriding the environment. The server's warnings print on stderr; `--json`
  prints its result on stdout.
- **Function lint**: before the build, each declared function's source (and the
  relative imports it reaches) is checked for what the host refuses at the first
  call — a `ctx.*` call whose capability the manifest lacks, a global the isolate
  does not have, a `ctx.warehouse` / `ctx.tx` write outside `destinations`; before
  the upload, with the target's database list, an `upsert` or `ctx.tx` on an
  engine that refuses it and a customer-warehouse write with no
  `customerWarehouseWrites` reason. The rules are `validate`'s, below. A finding
  fails the publish with exit `1` and names the file, line, call and fix; if the
  target will not list the databases, the engine half prints one warning and is
  skipped. **`--allow-function-lint`** is the way past a false positive: every
  finding becomes a warning that names its rule — please open an issue with it.

**`init-ci`** writes `.github/workflows/oxy-publish.yml` at the repo root: a
`build` job (no id-token) that runs `publish --build-only`, and an
environment-gated `publish` job whose only work is `publish --prebuilt` with
OIDC. `--promote` makes that publish go live and adds a `checks run` step after
it, when the manifest declares a check. It prints the `oxyc api …/publishers`
call that registers the workflow.

**`--app-env <dev-handle>`** moves one sandbox's build pointer instead of the
draft/live channel — see [Sandboxes](#sandboxes). It needs a staff credential
(never a publish token or OIDC) and is refused together with `--promote`: a
sandbox build is never promoted, so the loop is publish to the sandbox,
iterate, then publish the same tree to staging and promote that.

## Apps

Read-only: every request these make is a GET, against `/api/customer-apps/**`
(the app-admin role). `<app>` is `<org-slug>/<app-slug>` or an app UUID.

```bash
oxyc apps list [--org <slug>] [--published | --draft] [--builds] [--json]
oxyc apps show <app> [--json]
oxyc apps builds <app> [--json]
oxyc apps health [<app>] [--needs-attention] [--json]
oxyc apps usage <app> [--json]
oxyc apps drift [<app>] [--org <slug>] [--dir <path>] [--refresh] [--json]
```

- **`list`** — every app you can see, across organizations, walking the paged
  registry to its end. `STATE` is `live` when the app has a published build and
  `draft` when it has none. `SOURCE` is the repository the app is *registered*
  against. `--builds` adds the live build's id, the repository and commit it was
  built from, who published it and when — one extra request per app, six at a
  time.
- **`show`** — one app on one screen: the registry row, the live build, the
  draft build when it is newer than the live one, deployment integrity,
  availability, and the 7-day usage summary. `last promote` is when the promote
  or rollback action was last used; `oxyc publish --promote` does not update it,
  so it can be older than the live build.
- **`builds`** — the build history, newest first. `CHANNEL` marks the build each
  channel points at: `live`, `draft`, or both for one build.
- **`health`** — with no argument, the fleet table (published apps only):
  `down`, `degraded`, `not_measured`, `quiet` or `operational` per app, with the
  totals on stderr. `--needs-attention` keeps the first three. With `<app>`: the
  deployment-integrity checks, the availability windows, and the browser errors
  of the last 24 hours.
- **`usage`** — the 7-day summary, the visitors, and the tracked events by name.
- **`drift`** — see below.

Times are UTC. `--json` prints one document on stdout; a table goes to stdout and
the count line to stderr.

**An empty result and a failed request are different answers.** No apps, no
builds, no errors recorded: the command says so and exits `0`. A request that
failed exits with the code its status maps to (`4`, `5`, `7`, …). Where a
command makes several independent requests (`show`, `health <app>`,
`list --builds`, `drift`), it prints what it did read, marks what it did not as
`NOT READ` (`"failed": […]` / `live_build_error` in `--json`), and then exits
non-zero. These commands report state: a failing health check, a `down` app or
a drifted app is an answer and exits `0` — read it from the output.

The health route answers **503 with its report** when a check fails; `oxyc`
prints that as a failing report, not as an outage.

### `apps drift`

Compares what is live against source on **this machine**. For each app it takes
the live build's recorded repository and commit, finds a local checkout of that
repository (the ones `oxyc repos` and `oxyc path` report, or `--dir`), finds the
app's directory by the `oxy-app.json` whose `slug` and `orgSlug` match — at
`<app>/oxy-app.json` or `<app>/public/oxy-app.json`, never by folder name — and
counts the commits on the checkout's **current branch** that touch that
directory after the published commit.

| Result | Meaning |
| --- | --- |
| `in sync` | no commit on the current branch touches the app directory after the published commit |
| `N commits ahead` | that many do; they are listed |
| `unknown — <reason>` | the comparison could not be made |

`unknown` is never reported as `in sync`. Its reasons (`reason` in `--json`):

| `reason` | |
| --- | --- |
| `not_published` | nothing is live |
| `source_unrecorded` | the live build records no repository or no commit |
| `repo_not_checked_out` | the repository is not on this machine — clone it, or pass `--dir` |
| `commit_not_in_checkout` | the checkout does not have the published commit — `git fetch` there |
| `app_dir_not_found` | no tracked `oxy-app.json` declares this `slug` and `orgSlug` |
| `app_dir_ambiguous` | more than one does |
| `commit_not_on_branch` | the published commit is not an ancestor of the checked-out branch |
| `working_tree_dirty` | the app directory has uncommitted or untracked files |
| `git_failed` | `git log` or `git status` failed in the checkout, so the comparison could not be read |
| `request_failed` | the builds request failed (the command then exits non-zero) |

It runs read-only git commands and nothing else: it does not fetch, check out,
or refresh the index. The answer is therefore about the checkout as it is on
disk — a branch that is behind its remote reports fewer commits than the remote
has. With no `<app>`, every published app is compared, and `--dir` applies to
the apps whose source repository is that checkout's `origin`; with `<app>`,
`--dir` is used whatever its remote is.

An app published to two organizations from one directory (a staging org and a
production org) matches the manifest for only one of them; the other reports
`app_dir_not_found` and names the manifest it found with the other `orgSlug`.

## Checks

```bash
oxyc checks run <org-slug>/<app-slug>   # or an app UUID
oxyc checks run acme/dashboards --json --timeout 60
```

Runs every function the app declared `"check": true` (in `oxy-app.json`), one
POST per check to start its run, then polls each run to a terminal status —
`done`, `failed`, `cancelled`, or a client-side `timed_out` past `--timeout`
seconds (default 300, per check). A check **passes** when its run finishes
`done` and its answer does not parse to an object with `ok: false`.

Human mode prints one line per check to stderr as it lands (`✓ canary 4.1s` /
`✗ canary failed: <reason>`), suppressed by `--quiet`. `--json` prints one
object to stdout instead:

```json
{ "app": "acme/dashboards", "appId": "…", "checks": [{ "name": "canary", "runId": "…", "status": "done", "passed": true, "durationMs": 4123 }] }
```

**Auth** follows the same rule as `oxyc api`'s bearer surface, with one
difference: `checks run` talks to `/api/admin/**`, which is not the
`/external/api/*` surface, so a configured API key is sent explicitly as
`X-API-Key` only when **no bearer resolves** — never both. A bearer from
`oxyc login` (or `--token-env`) always wins when one is present.

**In CI, nothing is stored.** With neither a token nor an API key set, a job
holding `id-token: write` exchanges its GitHub OIDC token for the same
short-lived, app-scoped publish credential `oxyc publish` uses, and drives
`/api/customer-apps/**` instead — the same three handlers, mounted where a
publish token may reach them. The exchange returns the app id, so no lookup is
needed, and `<app>` must be `<org-slug>/<app-slug>` in this mode (a UUID names
an app the exchange cannot verify a publisher for). A publish token set in
`OXY_TOKEN` takes the same surface without minting.

What that credential may do is deliberately small, and enforced server-side
rather than by the client. An app-scoped token is **confined to its app**: it
reaches `/customer-apps/{its own id}/…` and the upload, and answers `404` for
everything else on that surface — another app's id, the registry listing, and
the fleet-wide rollups. That is the same answer an out-of-scope App Operator
gets, so it cannot learn which apps exist. Within its app it may publish, read,
and run a function **the manifest marks `"check": true`**; a function without
the flag is `403`. The
argument for allowing the run at all is narrow: a token that may publish
arbitrary code to an app can already cause anything that app can, so running
that app's own declared checks adds nothing. It would stop being true the moment
it admitted any function.

**A check runs against the app's live build** — the server resolves
`published_build_id`, else `draft_build_id`, for the pre-flight lookup and for
the execution alike. So `checks run` after a plain `oxyc publish` verifies the
*previously promoted* code, not the draft just uploaded. `oxyc init-ci` writes
the step only with `--promote`, where the build just published is the one the
check runs, and only when the manifest declares a check. **With `--app-env`**
it runs against that environment's own build instead — a sandbox's own
publish, or staging's — and the report gains `environment` and each check's
`invocationId`; the publish-token and OIDC paths are unchanged and work only
against production, since every non-production operation needs a staff
credential (see [Sandboxes](#sandboxes)).

With a publish token, `<app>` must be the **UUID** — resolving a slug means
listing every app, which such a token may not do. The OIDC path needs no id at
all (the exchange returns it) but does need `<org-slug>/<app-slug>`, since the
exchange is registered by slug.

**Exit codes:** `0` every check passed; `9` (`CHECK_FAILED`) at least one check
failed or timed out; `1` the app declares no checks; `5` the app was not
found; `4` no credential resolved, or the API rejected it (401/403 — an
expired 90-day API key reads this way, not as a missing one); `2` a bad
`--timeout`, or a slug where the credential needs a UUID; `7` (`UNAVAILABLE`)
the deployment answered the OIDC exchange without an `app_id`, meaning it
predates trusted checks.

## Sandboxes

```bash
oxyc env create <app> <name>                 # name is dev-<handle>, 1-12 chars a-z0-9-
oxyc env list <app>
oxyc env show <app> <name>
oxyc env delete <app> <name> [--yes] [--wait [seconds]]

oxyc publish --app-env <name> [--env <e>]    # see "Custom apps" above
oxyc fn call <app> <function> [--app-env <name>] [--data <json|@file|->]
oxyc checks run <app> --app-env <name>       # see "Checks" above

oxyc invocations list <app> [--app-env] [--build] [--function] [--limit]
oxyc invocations held <app> <invocation-id>
oxyc logs <app> [--app-env] [--invocation] [--request] [--hours] [--limit]
```

A **sandbox** is a named, short-lived `dev-<handle>` environment of one custom
app: its own build pointer, storage silo, secrets and Airhouse sibling, with
reads hitting production and most writes either isolated or *held* (recorded,
not performed — see `invocations held`). It starts with no build, is deleted
explicitly or after 7 idle days, and an app has at most 20. Full contract,
including what is **not** isolated (`ctx.oltp`, `ctx.warehouse`) and the other
gaps: `internal-docs/custom-app-sandboxes.md`.

The loop: `env create` → `publish --app-env` → `fn call` / `checks run
--app-env` → `invocations list` / `held` to read back what ran → iterate from
`publish` → `env delete --yes --wait` when done. `oxyc guide` has the same six
lines, meant to sit in an agent's context.

**`--app-env <environment>`** (`production`, `staging` or `dev-<handle>`,
validated client-side — exit `2` on a malformed name) is a *different axis*
from `--env`, which is the deployment: both can appear on the same command
(`oxyc fn call acme/store f --env dev --app-env dev-a1`). It is not a global
flag — only `publish`, `fn call`, `checks run`, `invocations list` and `logs`
take it. A publish-token credential is refused before any request for
`--app-env` other than `production` (exit `2`); `env`, `invocations` and
`logs` refuse one outright, for any environment, because sandbox management
is a staff console surface.

**`env delete`** without `--yes` asks on a terminal and exits `8` (refused)
off one, same as `oltp reset`. `--wait [seconds]` (default 120) polls until
the teardown finishes and prints `{"name","status":"deleted"}`; past the
deadline it exits `7` (retryable) rather than hanging.

**`fn call`** POSTs directly to `{target}/customer-apps/{org}/{app}/fn/{name}`
(no `/api`) — the response is Server-Sent Events, not JSON, so `oxyc` parses
the `log` / `data` / `done` / `error` frames itself. It exits `0` only when
the stream ends `done` with a 2xx status; a function that threw, or answered a
non-2xx, is a function-level failure (exit `1`, `ok: false` in the JSON), kept
distinct from a non-2xx on the POST itself (exit per the usual status mapping
— e.g. `EnvironmentHasNoBuild` before the stream starts). The
`x-oxy-invocation-id` response header becomes `invocationId` in the result.

**`invocations held`** is what the non-production write policy did *not* do —
production's would-be effect, recorded rather than performed. It is `[]` for a
production invocation, since nothing is held there.

## Workspace previews

```bash
oxyc preview create <branch> [--wait [seconds]] [--json]
oxyc preview list [--json]
oxyc preview show <branch> [--json]
oxyc preview delete <branch> [--yes] [--json]
oxyc preview checks <branch> [--json]

oxyc preview run <branch> <kind> <ref> [--variables <json|@file|->] [--read-live-only]
                  [--window-from <iso>] [--window-to <iso>] [--resource <name>...]
                  [--wait [seconds]] [--json]
oxyc preview runs list <branch> [--json]
oxyc preview runs show <run-id> [--wait [seconds]] [--json]
```

A **preview** opens a workspace branch on the real product, against real data,
without the branch being live — staff only. `create` compiles the branch's
head into a staging revision (or reuses a ready one for that commit) and is
idempotent; `--wait [seconds]` (default 120) blocks until the compile is
`ready` or `failed` instead of returning the `compiling` item the request
itself answers with, and exits `7` (retryable) past the deadline. There is no
single-preview route: `show` fetches `list` and filters it client-side, so its
`5` (not found) is synthesized here, not a literal 404.

The loop: `preview create --wait` → `preview checks` → `preview run` →
`preview runs show --wait` → iterate → `preview delete --yes` when done.

**`<kind>`** is `procedure` or `airway_sample` — the only two `POST
/previews/runs` accepts (exit `2` client-side on anything else, naming the
read-back verb). `transform_build` and `compare` runs are queued by the
server's own Airway change check, never started here; `preview runs list`
surfaces them alongside the kinds you *can* start, and `preview runs show
<run-id> --wait` reads any of the four back — a procedure's held steps, a
transform build's or compare's outcome, or a sample's ask and result.
`--variables` / `--read-live-only` are `procedure`-only; `--window-from` /
`--window-to` / `--resource` (repeatable) are `airway_sample`-only.

Every verb needs `{workspace}` resolved — `--workspace <id>`, exactly the way
`oxyc api {workspace}/...` resolves it; there is no second workspace
resolver. A deployment with `OXY_PREVIEW_RUNS` off answers every runs route
`404 preview_runs_disabled`, surfaced the same way any other server refusal
is: the server's own `code` rides along on the thrown error, and the exit
class follows the usual HTTP-status mapping (`400`/`409` → `6`, `404` → `5`).
Full contract: `internal-docs/workspace-previews.md`.

## Development commands

```bash
oxyc validate [-f <file>] [--json]
oxyc mcp
oxyc guide
oxyc skills install | list
oxyc cache clear
oxyc exit-codes
```

**`validate`** checks workspace YAML against `json-schemas/*.json`, which are
generated from the Rust config types. No network, no token.

| File | Schema |
| --- | --- |
| `config.yml` / `config.yaml` | `config.json` |
| `*.automation.yml`, `*.procedure.yml`, `*.workflow.yml` | `workflow.json` |
| `*.agentic.yml` | `agentic.json` |
| `*.app.yml` | `app.json` |
| `*.agent.test.yml` | `agent-test.json` |

Structural checks only. `oxy validate` additionally resolves `databases:` and
`llm.ref` against the loaded workspace, and wins where the two disagree.

Every **`oxy-app.json`** it finds is checked for data placement — the shape of
the data picks the store: facts and history in Airhouse (`ctx.airhouse`),
records in OLTP (`ctx.oltp`), files in `ctx.storage`, customer warehouses
read-only. These fail the run:

- `customerWarehouseWrites` that is not `{ "<database>": "<reason>" }`, or names
  a database missing from the function's `destinations`;
- `"airhouse": { "enabled": true }` in an app whose slug cannot derive a writer
  (`-` → `_`, 1–56 of `[a-z0-9_]`, starting with a letter; no `_` in the slug);
- `airhouseMigrations` without a `dir`, with one that does not exist, or with
  `migrations.dir`'s; and in its `*.sql`, `PRIMARY KEY`, `UNIQUE`,
  `CREATE INDEX`, `REFERENCES` or `FOREIGN KEY` (DuckLake has none), or a
  `CREATE`/`ALTER`/`DROP`/`INSERT`/`UPDATE`/`DELETE`/`COMMENT ON` target not
  written as `app_<writer>.<name>`.

Warnings print on stderr and do not fail: each customer-warehouse exception,
`CREATE SCHEMA` in a migration, and `ctx.secrets.set` holding state (a
`JSON.stringify` value, or a state-like key that is not a credential's). The SQL
checks are lexical; the server's parser is the authority.

Each declared **Oxy Function** is then linted — its entry and every relative
import it reaches, comments and strings masked — for the mistakes that work
locally and fail closed in production. These fail the run, each naming the
file, the line, the call and the fix:

- `capability` — a `ctx.<area>.<op>` call whose manifest capability the function
  does not declare: `ctx.secrets.set` → `secrets.write`, `ctx.email.send` →
  `email.send`, `ctx.org.*` → `org.read`, `ctx.storage.*` → `storage.read` /
  `storage.write` (`copy` needs both), `ctx.oltp.*` → `oltp.enabled`,
  `ctx.airhouse.*` → `airhouse.enabled`. The map is
  `src/publish/capabilities.ts`, held to the host's `FunctionCapabilities` by a
  Rust test.
- `isolate-global` — a value use of `Buffer`, `TextEncoder`, `TextDecoder`,
  `Blob`, `File`, `FormData`, `crypto.subtle` or `process.*`: the isolate is bare
  `deno_core` and each is a `ReferenceError` at runtime. A name the file declares
  itself (`const Buffer = …`, `import { TextEncoder } from "./polyfill"`) is not
  one; nor is a `typeof` guard or a type annotation.
- `destinations` — a `ctx.warehouse.insert` / `exec` / `upsert` or `ctx.tx` in a
  function with no `destinations`, or naming a database not in them.

The engine half — `upsert` off Postgres / DuckDB, `ctx.tx` off Postgres, a
customer-warehouse write with no `customerWarehouseWrites` reason, a write to
the org's OLTP store — needs the database list, which only the server has;
`validate` says so and `oxyc publish` runs it before the upload.

**`proxy`** forwards a local dev server's Oxy calls to a cloud target with your
login token attached. Defaults: side-effecting calls are **held**, tracking
events are **dropped**, auth endpoints reach the backend unauthenticated so
sign-in works, and the cached token never overrides a real browser session. A
production target (any host under `oxygen-hq.com`) is refused without `--yes`.
Each Oxy Function call carries a W3C trace — the SDK's, or one minted here — and
prints `↳ <status> fn <name>  request_id=…  trace_id=…`.

**`mcp`** serves the API over stdio as four generic tools — `oxy_routes`,
`oxy_schema`, `oxy_request`, `oxy_whoami` — rather than one per endpoint, so
those schemas cost ~2 KB per turn and reach endpoints added after this
package shipped. Two named exceptions trade that minimalism for validation an
agent needs to drive a loop unsupervised: the **sandbox loop**
(`oxy_env_create`/`list`/`show`/`delete`, `oxy_publish_sandbox`,
`oxy_fn_call`, `oxy_checks_run`, `oxy_invocations_list`/`held`, `oxy_logs`)
and **workspace previews** (`oxy_preview_create`/`list`/`show`/`delete`/
`checks`/`run`/`runs_list`/`run_show`). Each tool calls its CLI verb's own
request-building function, so a tool's result is the same JSON document that
verb's `--json` prints, and a failure carries the server's own error code
plus the exit-code class (`[exit 5 NOT_FOUND]`) the way `oxyc`'s exit code
would. `oxy_env_delete` / `oxy_preview_delete` need `confirm: true` (there is
no terminal to ask on inside an MCP server), and `oxy_publish_sandbox`
requires a `dev-<handle>` `appEnv` and refuses production, staging or a
promote client-side — it can never reach the live channel.

```bash
claude mcp add oxyc -- npx -y @oxy-hq/cli mcp --env production
```

**`guide`** prints a page to paste into `AGENTS.md` / `CLAUDE.md`.
**`skills install`** symlinks the six bundled Claude skills into
`~/.claude/skills`; it refuses to run from an `npx` cache, whose symlinks dangle
once npm reclaims it.

## Exit codes

```
0  success
1  failure with nothing more specific to say
2  usage error — a bad flag, a missing argument, a malformed value
4  not authenticated, or the token was rejected (401/403)
5  not found (404), or an unknown customer
6  the request was malformed (4xx other than 401/403/404)
7  unavailable — 5xx, a timeout, or the network failed. Retryable.
8  refused — the operation would have destroyed or overwritten something
9  a check ran and failed or timed out (`oxyc checks run`)
```

`4` almost always means the wrong `--env`. `7` is worth retrying; `6` never is.

## Environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `OXY_TOKEN` | — | bearer token, overriding the login cache (the CI path) |
| `OXY_API_KEY` | — | key for the `/external/api` surface |
| `OXY_CREDENTIALS_PATH` | OS config dir | the shared `credentials.json` |
| `OXYC_ORG` | `oxy-hq` | GitHub org the customer repos live in |
| `OXYC_CUSTOMER_TOPIC` | `oxy-customer` | topic that registers a customer repo |
| `OXYC_DOSSIER_ROOT` | `~/.oxyc/dossiers` | where customer clones go |
| `OXYC_REPO_ROOTS` | scanned | where to look for our own checkouts |
| `OXYC_TEMPLATE_DIR` | shipped `template/` | use a working copy instead |
| `OXYC_SCHEMAS_DIR` | shipped `json-schemas/` | use a working copy instead |
| `OXYC_SKILLS_DIR` | shipped `skills/` | use a working copy instead |
| `OXYC_SKILLS_TARGET` | `~/.claude/skills` | where `skills install` links |
| `OXYC_CACHE_DIR` | `<cache>/oxyc` | cache root |
| `OXYC_CACHE_TTL` | `3600` | seconds the customer listing is cached |
| `OXYC_LIST_LIMIT` | `1000` | max repos listed from GitHub |
| `OXYC_SEARCH_LIMIT` | `1000` | max pull requests searched by `activity` |
| `OXYC_DIFF_LINES` | — | cap on diff lines printed by `update` |
| `OXYC_DEBUG` | — | `1` keeps the stack on an unexpected throw |
| `OXYC_QUIET` | — | `1` is `--quiet` |
| `OXYC_DRY_RUN` | — | `1` makes `launch` print the command instead of running it |
| `NO_COLOR` / `FORCE_COLOR` | — | force plain / coloured output |

## Output contract

**stdout carries the response body and nothing else.** Progress, warnings,
errors and hints go to stderr, and a failure never exits `0`.

Attached to a TTY you get colour, aligned tables and a searchable picker.
Piped, you get markdown tables and no colour. Raw `api` response bodies are
byte-identical in both, so `| jq` always works.

## Development

```bash
pnpm install
pnpm build          # tsdown; `prebuild` runs codegen
pnpm test           # vitest; `pretest` builds
pnpm typecheck
pnpm lint           # biome
pnpm build:binary   # standalone executables (needs bun)
```

The package ships four directories — `dist`, `json-schemas`, `skills`,
`template` — and the last three are resolved at runtime relative to
`package.json`. `scripts/ci/verify-cli-package.mjs` checks the packed tarball
on every PR.

Maintainer's notes — release process, catalog generation, OpenAPI curation, CI
gates: [`internal-docs/oxy-api-cli.md`](../../internal-docs/oxy-api-cli.md).
