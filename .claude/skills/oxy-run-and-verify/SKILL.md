---
name: oxy-run-and-verify
description: Use when you need to run, start, or launch the Oxy / Oxygen app locally and look at a change in the real product — verify a UI or API change in the browser, confirm a fix works end to end, seed demo data, sign in as a member / org owner / staff / app operator / partner, screenshot a feature, or reproduce a reported UI bug with the Playwright MCP — or sign a browser in to a deployment (dev aip.dev.oxy.tech, staging, production) past its Google / GitHub / email login. Triggers on "run the app", "start the server", "start the stack", "just up", "just stacks", "port already in use", "another session's stack", "frontend-only", "check it in the browser", "see it working", "screenshot the page", "reproduce this bug", "sign in as", "log in as a member", "dev-login", "seed data", "state.json", "which persona", "503 needs_recompile", "x-oxy-assume-required", "aip.dev.oxy.tech", "log in to dev", "login-link", "can't log in". This is the project skill the built-in `run` skill looks for. Not for authoring committed regression flows (agentic-browser-test) or for unit/integration tests.
---

# Run the stack, sign in as a persona, verify

The loop is always the same, and it never involves `--local`, hand-started servers, or env edits:

1. **`just up`** — builds, starts, seeds, serves. Idempotent: re-run it after any change.
2. **Read `.oxy-dev/state.json`** — URLs, pids, log paths, personas, seeded workspace ids. **The ports are this checkout's** (slot 0 is `:5173`, slot 1 `:5183`, …): take `frontend_url` / `backend_url` from the file, never assume `:5173`.
3. **`browser_navigate` to `<frontend_url>/dev-login?as=<persona>&next=<path>`** (`dev_login_template` in state.json) — that one navigation is the whole sign-in.
4. Look, click, screenshot. Change code. Back to 1 (or nothing, for web-app edits — Vite hot-reloads).

**Here vs `agentic-browser-test`:** this skill is for *looking at* a change once. A regression you want kept belongs in a committed flow under `web-app/tests/agentic/flows/`; invoke that skill for it.

## Running it from Claude Code

- Run `just up` / `just status` / `just down` **with the Bash sandbox disabled**: they need the Docker socket, `ps`, and to bind ports. Sandboxed, the script exits at once naming the problem.
- **Other sessions' stacks are not in your way.** One stack per checkout, any number per machine: each checkout holds its own slot of ports, and they all share one database. Work in your own worktree (`cd "$(just wt-claim <branch>)"`) and `just up` there; never stop, restart or `--clean` a stack you did not start. `just stacks` shows who runs what.
- **A web-app-only change needs no backend of its own**: `just up --frontend-only` runs only Vite, proxying to a backend already running on this machine (no cargo build, no Docker). That backend is another checkout's binary, so use a full `just up` when the change touches `crates/**` or an API the web-app calls.
- A **cold** `just up` can outlast the 10-minute Bash timeout (a ~7 min first build, image pulls, then the seed compiling ~10 workspaces). Run it with `run_in_background: true` and wait for the completion notification. A warm run is mostly the seed step.
- The exit code is the contract. On failure the script prints the last 40 log lines and the log path, so read those before retrying anything.

## Commands

| Command | Does |
| ------- | ---- |
| `just up` | claims this checkout's slot → `cargo build -p oxy-server` → the **shared** data layer (Docker Postgres :15432, Airhouse :8080/:5445; started by `oxy start --db-only` only when not healthy) → `oxy serve --enterprise` on the slot's API + no-auth internal ports → `oxy dev seed --workspace-path ./examples --llm-keys --fixtures` (compiles + promotes every seeded workspace; copies env LLM keys into their secrets; builds the `pulse-demo` fixture app once; one seed at a time machine-wide) → Vite on the slot's port → writes state.json and prints three ready sign-in links |
| `just up --no-build` | reuse the existing binary (`$OXY_BIN`, else `target/debug/oxy`) |
| `just up --no-seed` | skip the seed (faster when data and compiled revisions are already there) |
| `just up --no-frontend` | API only, for `curl` checks |
| `just up --frontend-only` | Vite only, proxying to a backend already running on this machine (this checkout's, else the lowest slot's; `OXY_DEV_BACKEND_URL` names one) |
| `just up --restart` | restart backend + Vite even when healthy (after editing `.env`) |
| `just up --clean` | wipe the **shared** database volumes first; refused while another checkout's stack runs (`--force`) |
| `just status` | slot, pids, `/api/ready`, Vite + its `/api` proxy, containers, current log paths. Exits 1 when not serving |
| `just stacks` | every stack on this machine: slot, ports, health, checkout, branch; and the data layer |
| `just down` | stop this checkout's backend + Vite (`--release` also gives the slot back). `--db` also stops the shared containers, refused while another checkout's stack runs |

Reuse rules: the backend is kept when it is ours, ready, and the binary is unchanged; a build that produced a new binary restarts it. Vite is kept when it is ours and answering. The data layer is kept whenever it is healthy, whoever started it. A slot port held by anything else moves this checkout to the next free slot (the holder is printed); a data-layer port held by anything else fails the run. A second `just up` in the same checkout waits for the first, then reuses what it started.

## `.oxy-dev/state.json`

| Key | Use |
| --- | --- |
| `slot` | this checkout's slot N: API `3000+10N`, internal `3001+10N`, Vite `5173+10N` |
| `frontend_url`, `backend_url`, `internal_url`, `database_url` | where things are (always `127.0.0.1` for HTTP). `internal_url` is null with `--frontend-only` |
| `backend_shared_from` | null for this checkout's own backend; with `--frontend-only`, the slot (a number) whose backend Vite proxies to, or the string `"url"` when `OXY_DEV_BACKEND_URL` named it |
| `data_layer` | the shared Postgres URL and Airhouse admin URL every stack uses |
| `dev_login_template` | `<frontend_url>/dev-login?as={persona}&next={path}` |
| `token_template` | `<backend_url>/api/auth/dev-login?as={persona}` — GET returns `{token}` for `curl`, sets no cookie |
| `personas` | `{name: email}`; `persona_checks` holds what the server actually answered for each |
| `workspaces[]` | `org_slug`, `workspace_name`, `workspace_id`, `compiled`, `home_path` |
| `custom_apps[]` | `org_slug`, `app_slug`, `url` |
| `logs` | the current `build` / `backend` / `seed` / `frontend` log files |

Every run writes **new** timestamped logs under `.oxy-dev/logs/`. Take the path from `logs` (or `just status`), never a guessed or globbed name.

## Personas

| Persona | Signs in as | Standing | Verify with it |
| ------- | ----------- | -------- | -------------- |
| `member` | `amara.larsson@acme.test` | plain Acme member: no partner access, no team | the customer view: Home/launcher, custom apps, chat, data apps, automations as a non-admin; "you can't do that" paths |
| `owner` | `maya.nguyen@acme.test` | Owner of Acme | org + workspace settings (Members, Teams, Crew, Locations, Positions, Billing, Databases, Secrets, Apps), invitations |
| `staff` | first `OXY_GLOBAL_ADMINS` email in `.env` (debug builds only) | Global Admin + Owner of `local` | the admin console, and the IDE / chat / semantic model on the **Demo** workspace, which is the one with real seeded content |
| `operator` | `app-operator@oxy.local` | platform `app_operator` grant, **no** org membership | custom-apps admin (`/admin/apps`); out-of-scope admin pages answer *not found* by design |
| `partner` | `oliver.okafor@acme.test` | Acme member with partner-console access | `/partners` — Acme's clients (northwind, globex), apps, team, activity |

Pick the **least-privileged persona that should see the feature**, and use a second one to prove who shouldn't. Staff entering any org other than `local` needs an assume-role session, and that is a policy boundary, not a bug.

Seeded orgs: `local` (Demo `70787bb2-e11b-5488-b2c3-02e60d5fc7d3` from `./examples`, custom apps `oxy-starter` and `pulse-demo`), `acme` (partner; manages `northwind`, `globex`), `initech` (partner; manages `umbrella`), `vandelay` (unmanaged). Take workspace ids from `state.json`.

## Route map

`W` = `/<org_slug>/workspaces/<workspace_id>` (see `workspaces[].home_path`). All paths are on `frontend_url`.

| Surface | Path |
| ------- | ---- |
| Post-login dispatcher / org root | `/` · `/<org>` (each picks a workspace and redirects) |
| Home (launcher) | `W` or `W/home` |
| Chat | `W/threads` · `W/threads/<id>` |
| Automations | `W/automations` · `W/automations/<pathb64>` (`/workflows` is an alias) |
| Airway pipeline | `W/pipelines/<pathb64>` |
| Oxygen Nexus (formerly Factory) | `W/ide` → `W/ide/world-model` |
| Nexus: read-only files · SQL client · semantic model | `W/ide/files` (the served revision's files, read-only, with the repository panel when none is open) · `W/ide/database` (the SQL client: schema browser and ad-hoc queries; queries are no longer saved as files) · `W/ide/semantic` |
| Orchestrator · observability | `W/ide/coordinator/overview` · `W/ide/observability/traces` |
| Context graph | `W/context-graph` |
| Settings dialog | any `W` URL + `?settings=<section>`: `organization.{general,members,teams,crew,locations,positions,billing,integration}`, `workspace.{members,databases,airhouse,oltp,repositories,secrets,connections,apps}`, `preferences.appearance` |
| No org yet (pending invites, join by link) | `/onboarding` — orgs are created only by staff (`/admin/tenants?type=orgs`) or partners, each with a Ready `Default` workspace. An org with no ready workspace shows a "being set up" card at `/<org>`; `/<org>/onboarding` just redirects there |
| Admin console | `/admin` → `/admin/apps`; `/admin/{orgs,users,workspaces,workspace-health,tenants,compiles,internal-jobs,feature-flags,audit,app-admins,publish-tokens,airhouse,oltp}`; Global-Owner only: `/admin/billing/queue`, `/admin/airway` |
| Partner console | `/partners` · `/partners/{apps,team,activity}` |
| Custom app | `/customer-apps/local/oxy-starter/` (served by the backend through Vite's proxy) |
| Demo fixture app (functions + a 7-day release history) | `/admin/apps/local/pulse-demo?section=builds` (also `functions`, `issues`) as `operator`; the app itself at `/customer-apps/local/pulse-demo/`. Its history (r1..r4 promoted, r4 the bad release live, r5 on staging) and the traffic before the seed are seeded rows; the runs it queues, the `*/15` `snapshot` schedule and whatever you click are real. Present once built: to reset it, `oxy dev seed --clear` (or delete the app) and `just up` |

`<pathb64>` is standard base64 of the workspace-relative file path (`printf %s agents/analytics.agentic.yml | base64`). URL-encode `?` and `&` inside `next`: `next=/local/workspaces/<id>/home%3Fsettings%3Dorganization.members`.

## Reading what you see

| You see | It means | Do |
| ------- | -------- | -- |
| 503 with `"needs_recompile": true` / "This workspace isn't ready yet" | no promoted compiled revision; a compile was just enqueued | wait ~2 min, or `just up` (the seed compiles + promotes); a compile error is in the seed log |
| 403 with `x-oxy-assume-required`, or the assume-role dialog | a staff/operator identity inside a tenant workspace without an assume session | use the tenant's own persona (`owner` / `member`) |
| plain 403 | this persona lacks the role | the persona table, not the code, is usually wrong |
| dev-login **409** | persona not seeded | `just up` without `--no-seed` |
| dev-login **400** | unknown persona, or both `email` and `as` given, or a binary older than personas | fix the URL; `just up` rebuilds |
| dev-login **404** | dev-login disabled: no `OXY_GLOBAL_ADMINS` in `.env`, or a release binary | add it to `.env`, `just up --restart` |
| bounced to `/login` | no session / expired | navigate to the dev-login URL again |
| staff lands on `/admin/billing/queue` | that email is also in `OXY_OWNER`; a Global Owner is sent to admin from user-facing routes | use another persona for tenant pages |
| chat run errors on the provider / API key | no LLM key in the env when the seed ran (`just up` seeds with `--llm-keys`, storing keys as workspace secrets; a bare `oxy dev seed` does not) | put `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` in `.env`, `just up` |
| custom-app data 403 `origin not allowed` | a non-loopback origin | browse `frontend_url` (`127.0.0.1`) |
| backend refuses to boot, log says the key `does not open this database's` secrets | this checkout's `OXY_ENCRYPTION_KEY` differs from the one the shared database was sealed with | use the main checkout's `.env` (a worktree without one gets a symlink to it) |
| `waiting for seed lock` / `data-layer lock`, held by another checkout | another session's `just up` is seeding or starting the shared data layer | nothing: it goes on when that one finishes |

## After a change

| Changed | Do |
| ------- | -- |
| `crates/**` | `just up` (rebuilds; restarts the backend only if the binary changed; re-seeds) |
| `web-app/**` | nothing — Vite hot-reloads; reload the page if state is stale |
| `examples/**` YAML | `just up` (the seed recompiles + promotes Demo) |
| a migration | `just up` (the restart runs it) |
| `.env` | `just up --restart` |
| `pnpm-lock.yaml` | `just up` (frozen `pnpm install`, then restarts Vite) |
| data got weird | `just up --clean` |

## Playwright MCP

- **Sign-in** is one `browser_navigate` to the dev-login URL; the page posts, sets the session, and lands on `next`. To switch persona, navigate to `/dev-login?as=<other>` again. If the old identity lingers, `browser_close` and start over.
- Find elements with `browser_snapshot` (accessibility tree + refs) and prefer `data-testid`s. Use `browser_take_screenshot` for evidence; files land in `.playwright-mcp/` (gitignored).
- Diagnose with `browser_network_requests` (look for the 403/409/503 shapes above) and `browser_console_messages`.
- Missing browser: `pnpm --dir web-app/node_modules/@playwright/mcp exec playwright install chromium`.
- API-only check: `API=$(jq -r .backend_url .oxy-dev/state.json)`, `TOKEN=$(curl -s --noproxy '*' "$API/api/auth/dev-login?as=member" | jq -r .token)`, then `curl -s --noproxy '*' -H "Authorization: Bearer $TOKEN" "$API/api/orgs"`.

## A deployment (dev, staging, prod), not this machine

Everything above is local. A deployment has no dev-login, and its `/login` page is a dead end for an agent: Google refuses an automated browser, GitHub asks for a second factor, and the email field mails a magic link to an inbox you cannot read. **Never click any of them, and never ask for a password: there is none.** The sign-in is two steps:

1. In Bash: `oxyc login-link --env dev --next <path>`. Stdout is one URL (a single-use ticket, 5 minutes).
2. `browser_navigate` to that URL. It lands on `<path>`, signed in as the token.

`--env dev` is `aip.dev.oxy.tech`, `staging` is `aip.staging.oxy.tech`, `production` (the default) is `app.oxygen-hq.com`. A token works only on the deployment that minted it.

### No credential: exit `4`

| Where you run | Credential | It reaches | Who acts |
| ------------- | ---------- | ---------- | -------- |
| Cloud session whose environment has a network secret for the host, and `OXY_TOKEN=from-proxy` | the secret: the proxy adds it, no token is in the VM. It must be a personal token (`oxy_pat_`); `login-link` refuses a service account's | whatever the stored token reaches, staff standing included if the owner minted it in a browser with it | nobody, per session |
| Cloud session, no secret | an **agent token**, approved by device code (below) | the approver's org memberships, **never staff or partner standing**: the server refuses that on any device-code approval (`device_login.rs`) | the user, once, from any device |
| Your own machine (a browser opens) | an agent token approved in that browser: `oxyc tokens create --agent --env dev >~/.config/oxy/agent-token.env`, with `--standing` for staff reach. Not the engineer's own `oxyc login`. | the approver's reach, with standing only if they tick it | the person, once per token |

An agent token, without blocking on the person:

1. Start the mint with the Bash tool's `run_in_background: true`. The SessionStart hook prints this line for its env (dev in this repo):
   `mkdir -p ~/.config/oxy && (umask 077; oxyc tokens create --agent --no-browser --env dev >~/.config/oxy/agent-token.env)`
   With `--approver <their email>` (or `OXY_APPROVER` set on the environment) the request is also pushed to them.
2. About five seconds in, `Read` the task's output file. It holds the approval URL and code. The token goes to the file and never to you. Give the user the URL and code, and say what it is for.
3. Carry on, or end your turn. **Don't wait for the task before relaying the code**: it ends only when they approve (exit 0) or the request lapses (10 minutes, then mint again).
4. Every later Bash command sources the file (the hook arranged it), so step 1 of the sign-in now works.

`OXY_TOKEN` beats `oxyc login`'s cache, for every `--env`. With a production token in the file, every `--env dev` call is a 401, and `oxyc login --env dev` cannot fix it. Mint for the deployment you need, which replaces the file, or run that one command as `OXY_TOKEN= oxyc …`.

### Once signed in

- The session is the token: 12 hours at most, the token's reach and nothing more. A `403 session_required` (token management) or a `404` is the token's limit, not a bug.
- A dead link shows `[data-testid=token-login-error]`. Mint another link, not another token.
- Staff entering a tenant still need an assume-role session (`x-oxy-assume-required`), and an agent token has no staff standing to assume with. Use a page the approver's own membership reaches, or the network secret.
- Needs `oxyc` ≥ 0.8.0 (the cloud hook upgrades a stale one). A deployment too old to have the route: exit `5`.
- Details and the reasoning: `internal-docs/dev-sign-in.md` → "On a deployment".

## Gotchas

- **`127.0.0.1`, not `localhost`.** Vite binds IPv4 loopback only, and macOS resolves `localhost` to `::1` first.
- **Never `--local`**: it's the unmaintained no-auth single-workspace mode, and nothing you see there says anything about the product.
- **`RUSTC_WRAPPER=sccache` in your shell breaks the build** ("could not execute process `sccache`"; `.cargo/config.toml` forbids it). `just up` drops it for its own build; a bare `cargo build` does not.
- **`invalid archive member … librusty_v8.a` / `could not find native static library rusty_v8`**: the V8 prebuilt (~39 MB `.gz`) download was cut short. The sandbox proxy does this, and so do two cargo processes racing on `target/debug/gn_out`. Delete `target/debug/gn_out/obj/librusty_v8.*`, run `cargo clean -p v8`, and rebuild once, unsandboxed and alone.
- **`pnpm exec` can run an implicit `pnpm install`** when `node_modules` is behind the lockfile. For one-off tools call the binary directly: `web-app/node_modules/.bin/{tsc,vitest}`, and `node_modules/.bin/biome` from the repo root.
- **Don't set `OXY_DEV_LOGIN_EMAILS`**: it replaces the persona roster. **Don't export `OXY_ROLE`**: `oxy start` is one all-roles process, and `just up` strips it.
- **Never run `oxy start` (or `just dev-backend`) beside `just up` stacks**: it recreates the shared `oxy-postgres` container, dropping every stack's database, and removes it when it stops. `just up` runs backends as `oxy serve` and starts the containers with `oxy start --db-only` only when they are down. A separate `oxy dev seed` / `oxy dev compile` needs `OXY_DATABASE_URL=postgresql://postgres:postgres@localhost:15432/oxy`. `oxy dev` (seed, compile, test) is dev tooling: it exists in debug builds (the `dev-tools` cargo feature, on by default) and not in a release binary or image, so the release image cannot seed; its `<tag>-devtools` variant (published beside it for the release showcase) can.
- **The database is shared by every stack**, so the last seed wins the Demo workspace's compiled revision (from the seeding checkout's `examples/`), a branch's migration is applied for everyone, and periodic background work runs on whichever backend claims it. What your change shows can come from another checkout's `just up`: re-run yours before you trust a seeded or compiled result. DEVELOPMENT.md "What the stacks share" has the list.
