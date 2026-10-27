# Product Context

Injected into every Claude API call. Its only job: clarify what an agent
**cannot** cheaply retrieve from the code — renamed concepts, confusing
distinctions, product intent for triage, counterintuitive gotchas. **Not** a
feature catalog or changelog. If you'd find it by reading the module, cut it.

---

## Orientation

Oxy (user-facing brand: **Oxygen**): connect a warehouse and ask questions in
chat, where agents write and run SQL.

Deployment mode decides almost every bug report. **Cloud / enterprise**
(`oxy serve [--enterprise]`, `oxy start` for a Docker-Postgres dev box,
`ServeMode::Cloud`) is the real, maintained path — and **"run it locally" means
this**, so dev-vs-prod is *not* distinguishable by mode. **Legacy single-project**
(`--local`, `ServeMode::Local`) is one fixed workspace with **no auth** and is
**not maintained**; never design behavior around it.

---

## Terminology & renames (easy to get wrong)

- **Semantic Model** = formerly **Semantic Layer**. Copy says "semantic model"; the code's `SemanticLayer` / `semantic_layer` spellings are type/wire/storage contracts. Don't "finish" the rename there.
- **Automation** = formerly **Procedure / Workflow**. Canonical `.automation.yml` (`.procedure.yml` accepted); `.workflow.yml` is **no longer recognized**. Routes `/workflows/:id` and `/procedures` alias `/automations/:id`; Rust `Workflow*`/`Procedure*` names and `type: workflow` are wire/storage contracts.
- **Enroll** (kiosks, crew) — copy says "enroll"; API fields, routes and columns keep the old `enrol` spelling as contracts.
- **Oxygen Factory** = the Developer Portal / IDE (formerly Studio / Oxygen Builder / Oxygen Core), same `/ide` surface. The **Orchestrator Dashboard** replaced the **Coordinator**.
- **Agentic Agent** (`.agentic.yml`) = the multi-step FSM agent (kinds: **analytics**, **app builder**). **Builder Agent** = the file-editing copilot (chat **Build** mode) — distinct from the *app builder* kind.
- **Custom Apps Platform** (code-first React+Vite bundles, `oxyc publish`) is **not** YAML **Data Apps** (`.app.yml`). Copy says "custom app"; routes and hosts still say `customer-apps`. `analytics: false` silences **product** analytics only, and the `oxy-` event-name prefix is reserved (`400`s).
- **Oxy Functions** = server-side TypeScript handlers declared in `oxy-app.json` and shipped *inside* a Custom App bundle — **not** an Automation task or Data App `task`. Powers are **capability-gated and fail closed**; the isolate is **not Node** (no `Buffer`, `TextEncoder`, `process`…). A function that *catches* a failed host call still raises a signal, by design.
- **A bundle ships more than code** — OLTP/Airhouse migrations (applied to production **before every move of its pointer** — `oxyc promote`/`rollback`, the console's Promote draft and Roll back to…, a promoting publish — once each; a draft queues them for staging), secret keys, `webhook` blocks. **Production only serves a build staging has served** (`409 staging_never_served`; a sandbox's build goes through `oxyc promote --from dev-<handle>`). **Production's schedules are re-registered by `oxyc promote`/`rollback`, the console's Promote draft and Roll back to… (both now act through the plan routes), and a promoting publish**; *promote latest* and batch publish register none. A build a **sandbox agent token** published is refused by every promote and rollback (`409 draft_published_by_agent`) until a person republishes it. An unknown app or a function with no `webhook` block answers 404, not 401.
- **Production vs staging vs sandbox vs preview** — a custom app's **staging** host (also where an admin "draft preview" opens) sends every side effect somewhere safe: OLTP staging branch, separate storage, email only to the tester, held agent writes, refused automation runs. A **sandbox** is a per-developer/agent env with its own app schema. A staff **workspace preview** refuses anything that runs or writes, and background work never reads one. "My write didn't land" there is by design.
- **`ctx.oltp`** = a **third data plane** — per-org transactional Postgres, neither warehouse nor semantic model. Each writer owns one schema (an Airway pipeline's `raw_*` is analytics-agent-readable; an app's `app_*` is private). **Who invoked matters on one surface only**: `ctx.warehouse` on an `airhouse_managed` destination mints the *caller's* role, so a scheduled/webhook write that way is permission-denied while reads work. **Customer warehouses are read-only to apps** unless listed in `customerWarehouseWrites`.
- Four similarly-named engines: **airlayer** (semantic model), **Airform** (dbt-style modeling), **Airway** (ELT), **Airhouse** (warehouse + connector).
- **Verified Query** = a plain `.sql` file the analytics agent runs *as-is* when it matches, bypassing LLM SQL generation.
- **Two subdomain schemes** — an **org subdomain** boots the whole product for one org (apps under `/a/<slug>/`); a **custom-app subdomain** serves one app at its root.

---

## Roles (the same word means different things at different levels)

- **Platform standing is a grant, not a rank** — an `app_admins` row carries a **role** and a **scope** (all orgs, or a list). `is_app_admin` says only *that* someone is staff; nothing may authorize from it.
- **Global Owner** (`OXY_OWNER`) is root, incl. Billing and Global-admin management; **Global Admin** is Oxy ops minus those two. **App Operator** ships custom apps and **nothing else**, optionally org-scoped; out-of-scope answers **not found**, so an admin 404 may be a scope boundary.
- **Partner** (capability-gated, **not** org membership) — a distributor tier that creates and manages its downstream orgs and publishes their apps. No general platform reach.
- **Org Owner / Admin / Member** — tenant-internal only. Workspace role is derived separately, so an org Member can be a workspace Admin. Airhouse settings are open to every member *by design* (the credential is their own, read-only).
- **Frontline worker** — a PIN-holding crew identity, deliberately **not** org membership. App access is always an *explicit* grant, `ctx.user.email` is `null`, and a PIN works only on an enrolled kiosk — elsewhere it's refused exactly as a wrong PIN.
- **Places and reach** — an org-owned registry of nested locations and positions. Reach is resolved **before app code runs**; app helpers can only narrow it. A `scope: "reach"` semantic query naming no location-bound view is **refused**.
- **Per-app scope** — an app is org-wide (default) or restricted to **org teams**/members. **A grant narrows within an org, never widens into one.** Only `ctx.user` inside a Function is **verified** identity; a scheduled run executes as the org owner (`appRole: admin`).
- **Staff reach is not standing** — staff and partners entering a tenant need an **assume-role session** (60 min, reason logged). A staff "no permission" usually means *no active assume session*.

---

## Surfaces (for "which component is this?" triage)

- **Home** (`/`) — the rail's **Chat** was "Threads".
- **Onboarding is no longer the setup path** — orgs are provisioned by staff or a partner with a workspace; self-serve creation is gone. Landing on `/onboarding` means *no org*, not broken setup.
- **Developer Portal / IDE** (`/ide`) — protected `main` redirects edits to a new branch. Semantic surfaces (World Model, explorer, Metric Tree) read the **branch selected in the IDE**.
- **Admin → Workspace Health** — **opt-in**: no (or unparseable) `health_check:` block means the workspace is *absent*, not healthy. It reads the **compiled** model only, so nothing-compiled reports Degraded. **Anomalies don't vote**; `reconcile.yml` is the only correctness dimension.
- **Three "is this custom app OK?" answers** — a deployment-integrity check (green is compatible with every request failing); a traffic-derived verdict where **"no reading" is never healthy**; and the app's own `check: true` functions. Never poll an invented `/health`: a custom-app host answers `200` with the shell for *every* path.
- **Admin → Usage report** — weekly custom-app usage mailed to Global Owners/Admins, **only on its own Monday** (a mid-week deploy mails nobody) and never on schedule from a dev box. Staff opening an app count as its users.
- **World Model Graph** (`.world-model.yml`) ≠ **Context Graph** ≠ **Metric Tree**. Empty results are usually deliberate refusals: opportunities are sized per-unit only, and a peer cohort over an entity not bound to the locations registry is refused.
- **Experiments** (`/ide/experiments`, flask icon in the rail) — a list, then one page per experiment whose body is decided by its status: a pricing form for a draft (candidate designs priced side by side, ranked on **MDE per unit-day**; a group design's MDE is per *group*-day, so the raw numbers are not comparable), **operations only** while registered / live / closing, the report once reported or aborted (a result clearing a looser registered α reads *directional*, never *significant*). There is deliberately **no final effect number before `reported`** — the result route answers 409 and the UI shows a calm *sealed* state — so a missing number on a live run is correct, not a bug. A pricing **refusal** is the page's primary content, not an error. One draft design is edited in place; Register freezes it at the server's clock (a change is a duplicate).
- **Document libraries** — **visibility belongs to the document**, not a role, so reads work without org membership; review state is never access control.

---

## Components worth clarifying

- **Release timeline** (app console → Live build & releases) — **red is the failure rate alone**; latency and Web Vitals are amber at most (vitals are page-reported), and below the sample floors a release is "not comparable", never fine. Vitals show only for page loads that sent their build, so a release from before that runtime change shows none — by design.
- **Agentic Agent** — when the semantic model lacks a measure, it answers from what exists rather than handing off to the Builder Agent. "Data covers through *date*" comes from a view's `meta:` freshness — honest, not an under-report.
- **Pre-aggregation** — the **Pre-aggregated** badge follows whichever tier actually answered (rollup or warehouse fallback). Display surfaces serve a stale rollup during rebuild; a **monitor scan or explain declines** one. A draft pinned to a semantic branch bypasses rollups entirely.
- **Airway ELT** (`.airway.yml`) — schema migration is **additive only** (a wrong schema needs **Reset schema**, which drops tables); rewinding a cursor is a *separate* op that keeps tables and refuses where a re-pull would duplicate. **One run per pipeline per workspace**, DB-leased, so a busy pipeline makes the next run *wait* — not a failure.
- **Anomaly Monitoring** (`.monitor.yml` → **Insights Inbox**) — two silences that read as bugs: the current *incomplete* period is excluded, and a series needs **~8 seasonal cycles** of history before it's scanned. **Explain** files a driver that merely tracks its base as **mechanical**. A `notify:` block posts each anomaly to Slack **once**; an older binary **refuses a file that has the block**.
- **Authentication** — passwordless only (magic link, SSO, frontline PIN). **API credentials come in three kinds**: personal access tokens (workspace-scoped, never above the owner's current role; out-of-grant answers **not found**), org **service accounts** (not members, take no seat), and keyless GitHub-Actions CI tokens. Pre-existing keys show as **Legacy** and keep their old org-wide reach. `oxyc login-link` gives an agent a browser **token session** with that token's reach, so a missing page there is the token's scope, not a bug.

---

## Counterintuitive gotchas (high-cost, hard to guess)

- **Two ClickHouses, two trace-id spaces** — the *product* store (`OXY_OBSERVABILITY_BACKEND` / `OXY_CLICKHOUSE_*`) backs the tenant Traces console; *platform* telemetry is OpenTelemetry, exported only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. They **don't join**. Product capture has no default — unset (or a removed `duckdb`/`postgres` value) boots *disabled*, reading as "no traces"; spans surface ~30s late by design.
- **Agent traces belong to a workspace only if their root span said so** — a console reads only rows stamped with its workspace, so older or unstamped traces are in **nobody's** console ("traces missing", not an error).
- **DuckDB concurrent init** — two handles opening one file concurrently have SIGSEGV'd; the pool serializes init, so code outside it must too.
- **DuckLake has no indexes** — Airhouse DDL must avoid `CREATE INDEX`, `PRIMARY KEY`, `UNIQUE`; one such table makes the writer go inert.
- **ClickHouse reads everything after `VALUES` / `FORMAT` as row data** — a comment, `ON CONFLICT` or audit trailer appended to an INSERT fails it with `Code: 27`. Callers never decorate a warehouse statement; the connector places the tag per engine.
- **Empty-result warehouse queries** can panic in the shared Arrow bridge; each path must short-circuit. Oversized results and unbounded queries cap (10k rows) and flag **truncated**, so "missing rows" may be a cap.
- **An external source's `modified` timestamp can lie** (Toast doesn't advance it on card capture), so a repairing backfill must widen **both** ends. Only **one** component may rotate a QuickBooks refresh token — Intuit voids the old one, so two rotators deadlock into `invalid_grant`.
- **Workspace file discovery** prunes a path if **any** component is dot-prefixed or `target` / `node_modules` / `dist` / `build`, and drops a file whose **name** contains `.test.`. The IDE and the fleet use different listers, so a file only one lists resolves in the IDE and 404s on serve.
- **Two worker concepts** — the *durable task fleet* runs queued `TaskSpec` jobs; the *global singleton worker* (`OXY_INPROC_GLOBAL_WORKER`) drives schedules, monitor scans, pre-aggregation. With the singleton off, **schedule CRUD works but nothing fires**. Missed runs collapse to one.
- **Every Builder/analytics/workflow SSE stream must emit a terminal event** (`done` / `error` / `cancelled`), even on a failure before the orchestrator loop starts, or the frontend hangs forever.
- **LLM keys live in the workspace secrets store** in cloud (env only in legacy local), checked for the *selected agent's* provider only. Never force-redirect into the wizard over a missing key.
- **Production-only "missing workspace/topic" errors are instance affinity, not bad YAML** — per-request reads must come from the compiled workspace in Postgres. Symptoms look like content bugs ("Topic not found" with an empty list, custom-app `origin not allowed`). **Not compiled yet** must answer *retryable*.
- **Custom-app subdomains ride a server-side session cookie** separate from main-site login — logout must clear it server-side, and every OAuth provider must preserve the return-to-app destination.
- **An open custom app learns of a publish within ~5 min, but reloads by itself only when its effective mode is `auto`** — `OXY_CUSTOM_APP_UPDATES=auto`, or the app's own `performance.autoUpdate: true` (`"prompt"` escalates to it 24 h after detection); the platform's `off` beats the app. The injected runtime polls `__oxy/build`; under the default `observe` with no app override it only announces (`oxy:update-available`) — except that a lazy chunk which 404s after a publish **reloads the page even in observe**, discarding in-memory state, unless the app holds `defer()` / `useBusyWhile`, has an input focused, or cancels `oxy:chunk-stale`. In `auto` it also reloads at a safe moment (a navigation, 2 idle minutes, a return after 10+). An app opts out by cancelling the event or holding `defer()`, so a tab still on an old build is not necessarily a bug.

---

## Key file extensions

Most are self-describing; `oxy.yml` under `modeling/<project>/` maps a dbt target to an
Oxy connection. **`.monitor.yml`** and **`reconcile.yml`** (root-only singleton comparing
an Oxy measure to a live external source) share two non-obvious semantics:

- `timezone` only bites on a `type: datetime` time dimension; a `type: date` column is already a local date and is bucketed raw.
- `freshness` (`3d`) means "trust nothing newer than this horizon". On a `week`/`month` grain, a `freshness` under one full grain swings with the weekday, producing phantom drift for days at a time.
