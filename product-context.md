# Product Context

Injected into every Claude API call. Its only job: clarify what an agent
**cannot** cheaply retrieve from the code — renamed concepts, confusing
distinctions, product intent for triage, counterintuitive gotchas. **Not** a
feature catalog or changelog. If you'd find it by reading the module, cut it.

---

## Orientation

Oxy (user-facing brand: **Oxygen**) is the operating system for AI transformation:
connect a warehouse and ask questions in chat, where agents write and run SQL.

Deployment mode decides almost every bug report. **Cloud / enterprise**
(`oxy serve [--enterprise]`, `oxy start` for a Docker-Postgres dev box,
`ServeMode::Cloud`) is the real, maintained path — and **"run it locally" means
this**, so dev-vs-prod is *not* distinguishable by mode. **Legacy single-project**
(`--local`, `ServeMode::Local`) is one fixed workspace with **no auth** and is
**not maintained**; never design behavior around it.

---

## Terminology & renames (easy to get wrong)

- **Semantic Model** = formerly **Semantic Layer**. User-facing copy says "semantic model" everywhere; the code's `SemanticLayer` / `semantic_layer` spellings are type/wire/storage contracts. Don't "finish" the rename there.
- **Automation** = formerly **Procedure / Workflow**. Canonical `.automation.yml` (`.procedure.yml` accepted); `.workflow.yml` is **no longer a recognized file kind**. Canonical route `/automations/:id`, with `/workflows/:id` and `/procedures` as aliases. The Rust `Workflow*`/`Procedure*` names and `type: workflow` are wire/storage contracts.
- **Oxygen Factory** = the Developer Portal / IDE (formerly Studio / Oxygen Builder / Oxygen Core) — same `/ide` surface. The **Orchestrator Dashboard** replaced the old **Coordinator**.
- **Agentic Agent** (`.agentic.yml`) = the multi-step FSM agent (kinds: **analytics**, **app builder**), distinct from the single-shot sense of "agent." **Builder Agent** = the file-editing copilot (chat **Build** mode) — distinct from the *app builder* agentic agent.
- **Custom Apps Platform** (code-first React+Vite bundles, `oxyc publish`) is **not** YAML **Data Apps** (`.app.yml` dashboards). Copy says "custom app"; routes and hosts still say `customer-apps`. Local dev against live cloud data runs through an `oxyc proxy` sidecar. The `oxy-` event-name prefix is reserved (`400`s), and `analytics: false` silences **product** analytics only — a mounted signal and error counts still report, so operators can answer "is it up" regardless of a bundle's config.
- **Oxy Functions** = server-side TypeScript handlers declared in `oxy-app.json` and shipped *inside* a Custom App bundle (frontend and backend promote and roll back together) — **not** an Automation task or Data App `task`. Powers are **capability-gated in the manifest and fail closed**. Isolate gotchas: email templates must be **preact**; `ctx.fetch` decodes as UTF-8, so a binary body needs `encoding: "base64"` or silently corrupts; an app's **first** `storage.retention` rule changes how uploads are signed, so a non-SDK uploader starts failing opaquely; `ctx.tx()` is **Postgres-only**, and a `numeric` column must be cast (`amount::text`).
- **A bundle ships more than code** — the manifest also declares OLTP and Airhouse migrations (applied at publish, once each, refusing a file whose bytes changed), expected secret keys, and per-function `webhook` blocks. **Promoting a build from the admin console does not apply migrations**, so a console promote can put new code in front of tables that were never created. Webhook signatures are verified before app code runs; an unknown app or a function with no `webhook` block answers 404 rather than 401, so the anonymous route can't enumerate what exists.
- **`ctx.oltp`** = a **third data plane** — per-org transactional Postgres, neither the warehouse nor the semantic model. Each writer (a custom app, or an Airway pipeline landing raw extracts) owns one schema and can't see another's; a pipeline's `raw_*` schema is analytics-agent-readable, an app's `app_*` schema is private. A slug becomes a schema/role name, so underscores are rejected and an app can't be renamed or deleted while its writer is provisioned. **Who invoked matters on one surface only**: `ctx.warehouse` on an `airhouse_managed` destination mints the *caller's* workspace role and gives every background run Reader, so a scheduled, webhook or system-path function writing that way is permission-denied while its reads work. An app's own stores ignore the caller — `ctx.oltp` and `ctx.airhouse` (append-only facts, no DDL, nothing outside its schema) behave the same on a schedule as on a route. **Customer warehouses are read-only to apps** unless the function lists the database in `customerWarehouseWrites` with a reason, so a newly refused write is usually a missing exception, not a broken connector.
- Four similarly-named third-party engines: **airlayer** (semantic model), **Airform** (dbt-style modeling), **Airway** (ELT), **Airhouse** (warehouse + connector).
- **Verified Query** = a plain `.sql` file the analytics agent runs *as-is* when it matches the question, bypassing LLM SQL generation.
- **Two subdomain schemes** — an **org subdomain** boots the whole product pre-scoped to that org, serving its apps under `/a/<slug>/`; a **custom-app subdomain** serves one app at its own root.

---

## Roles (the same word means different things at different levels)

- **Platform standing is a grant, not a rank** — a row in `app_admins` carries a **role** (capability preset) and a **scope** (all orgs, or a list). `is_app_admin` says only *that* someone is staff, so nothing may authorize from it.
- **Global Owner** (`OXY_OWNER` → `is_owner`) is root, incl. Billing and Global-admin management; **Global Admin** is Oxy ops — most of admin plus every custom app, but not those two. **App Operator** ships custom apps and **nothing else**, optionally scoped to named orgs; an out-of-scope request answers **not found** rather than *not allowed*, so a 404 in an admin surface may be a scope boundary, not a missing row.
- **Partner** (capability-gated, **not** an org membership) — a distributor tier owning downstream orgs: for those only, it creates orgs, manages members, publishes apps and names each app's audience. No general platform reach.
- **Org Owner / Admin / Member** (`org_members.role`) — tenant-internal only, **no** platform reach. Workspace role derives via `EffectiveWorkspaceRole`, so an org Member who is a workspace Admin still reaches Databases / Secrets / Apps / API Keys. Airhouse settings stay open to every member *by design* — the credential it mints is their own, read-only and time-limited.
- **Frontline worker** — a PIN-holding crew identity, deliberately **not** org membership, so enrolling one never grants workspace reach. App access is always an *explicit* grant (org-wide visibility never reaches them), `ctx.user.email` is `null`, and a PIN authenticates only on an enrolled kiosk — elsewhere it's refused exactly as a wrong PIN is. Suspending revokes login but keeps the row, so completed work stays attributable.
- **Places, positions and "reach"** — the org owns one registry of nested locations (each with a per-system identifier map, so a store known by three ids reconciles) and of positions. An assignment resolves to the places a caller may act on **before any app code runs**, and app-side helpers can only narrow it. A semantic query asked with `scope: "reach"` is **refused** when it names no location-bound view.
- **Per-app scope** — an app is visible org-wide (default) or restricted to named **org teams**/members, and a **per-app admin** role can extend app-admin rights through a team to a non-officer. **A grant narrows within an org, never widens into one** — a non-member holding one is denied. Only `ctx.user` inside a Function is **verified** identity (`appRole` its one gateable field); the browser context carries display identity the viewer can edit, and a scheduled run executes as the org owner, reading `appRole: admin`.
- **Staff reach is not standing** — staff and partners entering a tenant workspace need an explicit **assume-role session** (60 min, non-renewable, reason logged). A staff-facing "you don't have permission" usually means *no active assume session*.

---

## Surfaces (for "which component is this?" triage)

- **Home / HQ launcher** (`/`, `/home`) — apps-first landing listing only the custom apps the viewer can open. The rail's **Chat** entry was formerly "Threads"; **Ask Oxygen** (⌘K) is a right-side drawer.
- **Onboarding is no longer the setup path** — orgs are provisioned (admin console, partner console, or an operator) and arrive with a workspace; self-serve creation is gone. `/onboarding` is a "you're not in an organization yet" page and Home never redirects into the wizard, so landing there means *no org*, not broken setup.
- **Developer Portal / IDE** (`/ide`) — protected `main` auto-redirects edits to a new branch. Semantic surfaces (World Model, explorer, Metric Tree) read the **branch selected in the IDE**, not always `main`.
- **Admin → Workspace Health** — **opt-in per workspace**: no `health_check:` block in `config.yml`, or an unparseable one, means the workspace is never evaluated and is *absent* from the table rather than shown healthy. Only an unhealthy *transition* pages Slack. Health reads the **compiled** model and won't fall back to a node's working copy, so nothing-compiled reports Degraded. **Anomalies are displayed but do not vote** (a holiday scores High while nothing is broken); `reconcile.yml` is the only correctness dimension.
- **Two different "is this custom app OK?" answers** — a deployment-integrity check reporting `oxy_app_health: pass|fail` (not `healthy`/`unhealthy`, which a `Contains` matcher misreads), whose green is compatible with every request failing; and a traffic-derived availability verdict — down / degraded / not measured / quiet / operational — where **"no reading" is never healthy** and a 4xx doesn't dent the score. Neither is reachable by polling an invented `/health`: a custom-app host answers `200` with the app shell for *every* path.
- **World Model Graph** (Globe icon), driven by `.world-model.yml`, is distinct from the **Context Graph** and the **Metric Tree**. Its refusals are deliberate, so an empty result is usually right: **opportunities** are sized per-unit only (`gap × volume`, never raw totals) over significant segments, and no denominator means refused; a **peer cohort** over an entity not bound to the locations registry is refused rather than given a population-wide median.
- **Document libraries** — **visibility belongs to the document**, not to a role, so reads work for someone holding no org membership; review state is a separate axis from publication, never access control.

---

## Components worth clarifying

- **Agentic Agent** — when the semantic model lacks a measure it needs, it answers from what exists rather than handing off to the Builder Agent mid-run. It is **freshness-aware** — prompts carry per-source coverage from a view's `meta:` contract, so "data covers through *date*" is honest, not an under-report.
- **Pre-aggregation** — a **Pre-aggregated** badge means a local Parquet rollup answered instead of the warehouse (stale data under it is a freshness bug); every read falls back to the warehouse, so the badge follows the tier that actually answered. Display surfaces serve a stale rollup during a rebuild, but a **monitor scan or explain declines a stale one** — its number becomes an assertion that can page. The panel lists what was **declared**, not built: **Not built** / **Built elsewhere** are statuses, not errors.
- **Airway ELT** (`.airway.yml`) — credentials live in the secret manager, never the YAML; schema migration is **additive only**, so a pipeline on a wrong schema needs an explicit **Reset schema**; a retry resumes from its cursor rather than re-pulling history. **One run per pipeline per workspace**, DB-leased on every path (automation steps included), because concurrent runs share one cursor. A busy pipeline makes the next run *wait*, and a submit joins the run already queued (ten **Run now** clicks = one run), so a schedule tick that joins is not a failure; `allow_concurrent_runs: true` opts out. A source's declared **contract** is gated by an admission policy resolved at *queue* time, so a later edit never rewrites past runs.
- **Metric Tree & Anomaly Monitoring** — a `.monitor.yml` watches a measure over time (per-segment via `filters`/`group_by`); anomalies land in the **Insights Inbox** with AI root-cause. The **Metric Tree** (a Semantic Model IDE tab) separately decomposes a top-line metric into drivers. Two deliberate silences that read as bugs: scans exclude the current *incomplete* period, and a series isn't scanned until it has **~8 seasonal cycles of history** (≈8 weeks daily), so a new segment stays quiet rather than reporting its ramp. Severity is *distance past the envelope edge*; too few same-phase points means no opinion, floored at **Medium**. Many segments breaching one bucket group into a **cohort**, and a `calendar:` block only *labels* one, never filters. **Explain** compares the same phase one cycle back (Monday vs prior Monday), scopes to the segment that fired, and files a driver that merely tracks its base as **mechanical** — neither cause nor offset.
- **Authentication** — passwordless: magic link, Google / Okta / GitHub SSO, or a frontline PIN; legacy password auth removed. One token covers browser, custom-app subdomain and `oxyc login`. CI publishes use a long-lived publish-scoped token owned by whoever minted it, not a session.

---

## Counterintuitive gotchas (high-cost, hard to guess)

- **Two ClickHouses, two audiences, two trace-id spaces** — the *product* store (`OXY_OBSERVABILITY_BACKEND` / `OXY_CLICKHOUSE_*`) backs the tenant-facing Traces console and holds agent/automation spans under its own ids; *platform* telemetry is OpenTelemetry ids on every span and log line of `serve` / `start` / `worker`, exported only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set (log export a further opt-in, since the collector tails stdout). A product span and the log line inside it carry **different** trace ids and don't join, so a missing *platform* trace is never fixed with `OXY_CLICKHOUSE_URL`. Dev carries no product traffic, so request-flow questions must be asked of prod.
- **Observability capture is ClickHouse-only with no default in any mode** — unset means capture is off, and a removed `duckdb` / `postgres` / `airhouse` label errors at boot and starts *disabled*, reading as "no traces". Timestamps must be served ISO-8601 UTC or the browser mis-parses them (render crash), and trace queries need hard time/size caps — an unbounded scan took the backend offline. Spans surface ~30s later by design.
- **DuckDB concurrent init** — two handles opening the same file concurrently have caused SIGSEGV; the pool serializes init, so code opening DuckDB outside it must too.
- **DuckLake has no indexes** — DDL against Airhouse/DuckLake tables must avoid `CREATE INDEX`, `PRIMARY KEY`, `UNIQUE`; a table carrying one fails and the writer goes inert from there on.
- **ClickHouse reads everything after `VALUES` / `FORMAT` as row data**, so anything appended to an INSERT parses as a row and the whole insert fails with `Code: 27` — a comment, `ON CONFLICT`, an audit trailer (one host-added trailer broke every `ctx.warehouse.insert` this way). A caller therefore never decorates a warehouse statement itself; it hands the statement and its tag to the connector, which places the tag per engine.
- **Empty-result warehouse queries** can panic in the shared Arrow bridge (DuckDB / Snowflake / MotherDuck / connectorx); each path must short-circuit its empty shape. Oversized results hit a memory backstop and unbounded semantic/SQL-IDE queries cap at 10k rows — both flag **truncated**, so "missing rows" may be a cap, not a query bug.
- **An external source's own `modified` timestamp can lie** — Toast doesn't advance an order's modified time when a card is captured, so such resources re-read a trailing window every run, and a backfill repairing them must widen **both** ends of its range since the axis it filters on is the untrustworthy one. Relatedly, only **one** component may rotate a QuickBooks refresh token: Intuit voids the old one on issue, so two rotators deadlock into `invalid_grant`.
- **Workspace file discovery skips by COMPONENT at any depth, and by file NAME separately** — two rules, and conflating them has split the two enumerations twice. A path is pruned if **any** component is dot-prefixed or is `target` / `node_modules` / `dist` / `build` (stray copies under these are the spurious "duplicate name" error). Separately, a file whose **name** contains `.test.` is a fixture and drops, while a real file under a fixtures *directory* does not. The IDE reads the working-copy lister and the stateless fleet reads the compile walker, so a file only one lists resolves in the IDE and 404s on serve.
- **Two distinct worker concepts** — the *durable task fleet* (in-process by default, standalone via `oxy worker`) runs queued `TaskSpec` jobs; the *global singleton worker* (`OXY_INPROC_GLOBAL_WORKER`) drives schedules, monitor scans, pre-aggregation. Toggling one doesn't affect the other, and with the singleton off **schedule CRUD works but nothing ever fires**. Missed runs collapse to one.
- **Every Builder/analytics/workflow SSE stream must emit a terminal event** (`done` / `error` / `cancelled`) — even when it fails before the orchestrator loop starts (a broken `.view.yml`) — or the frontend hangs forever.
- **Mode-dependent LLM-key check** — "is a key set?" must read the **workspace secrets store** in cloud (env vars only in legacy local), and only for the *selected agent's* provider. Home deliberately doesn't check on the common path, so a keyless workspace surfaces on the first failed message; never force-redirect into the wizard over it.
- **Production-only "missing workspace/topic" errors are instance affinity, not bad YAML** — anything read per request must come from the compiled workspace in Postgres, not a working copy present only on the owning instance. Symptoms look like content bugs: "Topic not found" with an empty topic list, "failed to read workspace", rejected webhooks, custom-app `origin not allowed`, a pipeline or delegated `.sql` file that exists reading as missing. **Not compiled yet** must answer *retryable*, distinct from genuinely-not-found.
- **Azure OpenAI routes through the OSS path** (a history of agentic incompatibilities) — re-test agentic flows after any LLM-routing change.
- **Custom-app subdomains ride a server-side session cookie, separate from main-site client login state** — logout must clear it server-side, and every OAuth provider (not just magic-link) must preserve the return-to-app destination, or the two disagree on whether you're signed in.

---

## Key file extensions

Most are self-describing; `oxy.yml` under `modeling/<project>/` maps a dbt target to an
Oxy connection. **`.monitor.yml`** (watches a measure) and **`reconcile.yml`**
(root-only singleton comparing an Oxy measure against a live external source, feeding
workspace health's **Reconciliation** dimension) share two semantics you can't guess:

- `timezone` only bites on a `type: datetime` time dimension; a `type: date` business-date column is already a local calendar date, bucketed raw, so it's inert there.
- `freshness` (`3d`) means "trust nothing newer than this horizon," so a lagging warehouse's unloaded buckets stop reading as a collapse. But on a `week`/`month` grain a `freshness` under one full grain buys a settle time that swings with the weekday, so a check reconciles cleanly for days and then reports drift that isn't there.
