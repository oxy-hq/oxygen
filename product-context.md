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
- **Oxy Functions** = server-side TypeScript handlers declared in `oxy-app.json` and shipped *inside* a Custom App bundle — **not** an Automation task or Data App `task`. Powers are **capability-gated and fail closed**. The isolate is **not Node** (no `Buffer`, `TextEncoder`, `crypto.subtle`, `process`…); email templates must be **preact**; `ctx.fetch` decodes as UTF-8, so binary needs `encoding: "base64"`; `ctx.tx()` is **Postgres-only** and a `numeric` column must be cast (`amount::text`). A function that *catches* a failed host call still raises a signal, by design.
- **A bundle ships more than code** — OLTP and Airhouse migrations (applied at publish, once each, refusing a file whose bytes changed), expected secret keys, `webhook` blocks. **Promoting a build from the admin console does not apply migrations.** An unknown app or a function with no `webhook` block answers 404, not 401, so the anonymous route can't enumerate.
- **`ctx.oltp`** = a **third data plane** — per-org transactional Postgres, neither warehouse nor semantic model. Each writer owns one schema (an Airway pipeline's `raw_*` is analytics-agent-readable; an app's `app_*` is private). **Who invoked matters on one surface only**: `ctx.warehouse` on an `airhouse_managed` destination mints the *caller's* role and gives background runs Reader, so a scheduled/webhook function writing that way is permission-denied while reads work; `ctx.oltp` and `ctx.airhouse` ignore the caller. **Customer warehouses are read-only to apps** unless listed in `customerWarehouseWrites`, so a newly refused write is a missing declaration, not a broken connector.
- Four similarly-named engines: **airlayer** (semantic model), **Airform** (dbt-style modeling), **Airway** (ELT), **Airhouse** (warehouse + connector).
- **Verified Query** = a plain `.sql` file the analytics agent runs *as-is* when it matches, bypassing LLM SQL generation.
- **Two subdomain schemes** — an **org subdomain** boots the whole product scoped to that org, apps under `/a/<slug>/`; a **custom-app subdomain** serves one app at its root.

---

## Roles (the same word means different things at different levels)

- **Platform standing is a grant, not a rank** — an `app_admins` row carries a **role** and a **scope** (all orgs, or a list). `is_app_admin` says only *that* someone is staff; nothing may authorize from it.
- **Global Owner** (`OXY_OWNER`) is root, incl. Billing and Global-admin management; **Global Admin** is Oxy ops minus those two. **App Operator** ships custom apps and **nothing else**, optionally org-scoped; out-of-scope answers **not found**, so an admin 404 may be a scope boundary.
- **Partner** (capability-gated, **not** org membership) — a distributor tier that creates and manages its downstream orgs and publishes their apps. No general platform reach.
- **Org Owner / Admin / Member** — tenant-internal only. Workspace role is derived separately, so an org Member who is a workspace Admin still reaches Databases / Secrets / Apps / API Keys. Airhouse settings are open to every member *by design* (the credential is their own, read-only).
- **Frontline worker** — a PIN-holding crew identity, deliberately **not** org membership. App access is always an *explicit* grant, `ctx.user.email` is `null`, and a PIN works only on an enrolled kiosk — elsewhere it's refused exactly as a wrong PIN.
- **Places and reach** — an org-owned registry of nested locations (each with a per-system id map) and positions. Reach is resolved **before app code runs**; app helpers can only narrow it. A `scope: "reach"` semantic query naming no location-bound view is **refused**.
- **Per-app scope** — an app is org-wide (default) or restricted to **org teams**/members. **A grant narrows within an org, never widens into one.** Only `ctx.user` inside a Function is **verified** identity; a scheduled run executes as the org owner (`appRole: admin`).
- **Staff reach is not standing** — staff and partners entering a tenant need an **assume-role session** (60 min, reason logged). A staff "no permission" usually means *no active assume session*.

---

## Surfaces (for "which component is this?" triage)

- **Home / HQ launcher** (`/`, `/home`) — apps-first; the rail's **Chat** was "Threads"; **Ask Oxygen** (⌘K) is a drawer.
- **Onboarding is no longer the setup path** — orgs are provisioned by staff or a partner with a workspace; self-serve creation is gone. Landing on `/onboarding` means *no org*, not broken setup.
- **Developer Portal / IDE** (`/ide`) — protected `main` redirects edits to a new branch. Semantic surfaces (World Model, explorer, Metric Tree) read the **branch selected in the IDE**.
- **Admin → Workspace Health** — **opt-in**: no (or unparseable) `health_check:` block means the workspace is *absent*, not healthy. It reads the **compiled** model only, so nothing-compiled reports Degraded. **Anomalies don't vote**; `reconcile.yml` is the only correctness dimension.
- **Three "is this custom app OK?" answers** — a deployment-integrity check (green is compatible with every request failing); a traffic-derived verdict where **"no reading" is never healthy**; and the app's own `check: true` functions. Never poll an invented `/health`: a custom-app host answers `200` with the shell for *every* path.
- **World Model Graph** (`.world-model.yml`) ≠ **Context Graph** ≠ **Metric Tree**. Empty results are usually deliberate refusals: opportunities are sized per-unit only, and a peer cohort over an entity not bound to the locations registry is refused.
- **Document libraries** — **visibility belongs to the document**, not a role, so reads work without org membership; review state is never access control.

---

## Components worth clarifying

- **Agentic Agent** — when the semantic model lacks a measure, it answers from what exists rather than handing off to the Builder Agent. Prompts carry per-source freshness from a view's `meta:`, so "data covers through *date*" is honest, not an under-report.
- **Pre-aggregation** — the **Pre-aggregated** badge follows whichever tier actually answered (rollup or warehouse fallback). Display surfaces serve a stale rollup during rebuild; a **monitor scan or explain declines** one.
- **Airway ELT** (`.airway.yml`) — credentials live in the secret manager. Schema migration is **additive only** (a wrong schema needs **Reset schema**, which drops tables); rewinding a cursor is a *separate* op that keeps tables and refuses where a re-pull would duplicate. **One run per pipeline per workspace**, DB-leased, so a busy pipeline makes the next run *wait* and a submit may join a queued run — not a failure.
- **Anomaly Monitoring** (`.monitor.yml` → **Insights Inbox**) — two silences that read as bugs: the current *incomplete* period is excluded, and a series needs **~8 seasonal cycles** of history before it's scanned. **Explain** compares the same phase one cycle back and files a driver that merely tracks its base as **mechanical**.
- **Authentication** — passwordless only (magic link, SSO, frontline PIN). One token covers browser, custom-app subdomain and `oxyc login`; CI publishes use a publish-scoped token, not a session.

---

## Counterintuitive gotchas (high-cost, hard to guess)

- **Two ClickHouses, two trace-id spaces** — the *product* store (`OXY_OBSERVABILITY_BACKEND` / `OXY_CLICKHOUSE_*`) backs the tenant Traces console; *platform* telemetry is OpenTelemetry, exported only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. A product span and the log line inside it **don't join**, so a missing platform trace is never fixed with `OXY_CLICKHOUSE_URL`. Dev carries no product traffic — ask request-flow questions of prod.
- **Observability capture is ClickHouse-only with no default** — unset means off, and a removed `duckdb` / `postgres` / `airhouse` value boots *disabled*, reading as "no traces". Spans surface ~30s later by design.
- **DuckDB concurrent init** — two handles opening one file concurrently have SIGSEGV'd; the pool serializes init, so code outside it must too.
- **DuckLake has no indexes** — Airhouse DDL must avoid `CREATE INDEX`, `PRIMARY KEY`, `UNIQUE`; one such table makes the writer go inert.
- **ClickHouse reads everything after `VALUES` / `FORMAT` as row data** — a comment, `ON CONFLICT` or audit trailer appended to an INSERT fails it with `Code: 27`. Callers never decorate a warehouse statement; the connector places the tag per engine.
- **Empty-result warehouse queries** can panic in the shared Arrow bridge; each path must short-circuit. Oversized results and unbounded queries cap (10k rows) and flag **truncated**, so "missing rows" may be a cap.
- **An external source's `modified` timestamp can lie** (Toast doesn't advance it on card capture), so such resources re-read a trailing window and a repairing backfill must widen **both** ends. Only **one** component may rotate a QuickBooks refresh token — Intuit voids the old one, so two rotators deadlock into `invalid_grant`.
- **Workspace file discovery: COMPONENT rule vs NAME rule** — a path is pruned if **any** component is dot-prefixed or `target` / `node_modules` / `dist` / `build` (strays there cause spurious "duplicate name"); separately, a file whose **name** contains `.test.` drops. The IDE and the fleet use different listers, so a file only one lists resolves in the IDE and 404s on serve.
- **Two worker concepts** — the *durable task fleet* runs queued `TaskSpec` jobs; the *global singleton worker* (`OXY_INPROC_GLOBAL_WORKER`) drives schedules, monitor scans, pre-aggregation. With the singleton off, **schedule CRUD works but nothing fires**. Missed runs collapse to one.
- **Every Builder/analytics/workflow SSE stream must emit a terminal event** (`done` / `error` / `cancelled`), even on a failure before the orchestrator loop starts, or the frontend hangs forever.
- **LLM-key check is mode-dependent** — cloud reads the **workspace secrets store** (env only in legacy local), for the *selected agent's* provider only. Never force-redirect into the wizard over a missing key.
- **Production-only "missing workspace/topic" errors are instance affinity, not bad YAML** — per-request reads must come from the compiled workspace in Postgres. Symptoms look like content bugs ("Topic not found" with an empty list, custom-app `origin not allowed`, a delegated `.sql` "missing"). **Not compiled yet** must answer *retryable*.
- **Azure OpenAI routes through the OSS path** — re-test agentic flows after any LLM-routing change.
- **Custom-app subdomains ride a server-side session cookie** separate from main-site login — logout must clear it server-side, and every OAuth provider must preserve the return-to-app destination.

---

## Key file extensions

Most are self-describing; `oxy.yml` under `modeling/<project>/` maps a dbt target to an
Oxy connection. **`.monitor.yml`** and **`reconcile.yml`** (root-only singleton comparing
an Oxy measure to a live external source) share two non-obvious semantics:

- `timezone` only bites on a `type: datetime` time dimension; a `type: date` column is already a local date and is bucketed raw.
- `freshness` (`3d`) means "trust nothing newer than this horizon". On a `week`/`month` grain, a `freshness` under one full grain swings with the weekday, producing phantom drift for days at a time.
