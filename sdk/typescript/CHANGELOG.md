# Changelog

All notable changes to the Oxy TypeScript SDK will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.23.1] - 2026-10-30

### Fixed

- **`<OxyShell>`'s rail tooltip is readable in a dark app without a `.dark`
  class.** The tooltip's text is the host's `--foreground`, but in the light
  scope its box was the fixed `--zinc-200` (#e4e4e7). A dark-only app that
  never sets `.dark` got near-white text on light grey (1.08:1, measured on
  two production apps). The box is now the host's `--muted` in both scopes,
  falling back to `--zinc-200` and then #e4e4e7. The web-app's light `--muted`
  is `--zinc-200`, so the product does not change. An app gets this by
  upgrading and republishing.

## [2.23.0] - 2026-10-28

### Changed

- **`<OxyShell>` on a phone: a menu instead of the icon rail.** Below 768px
  the 48px rail is hidden and a menu button at the start of the top bar opens
  a left drawer with the same entries — HQ, Chat, every app — each with its
  name, as 44px rows at full opacity, the current app marked. The rail's names
  were hover tooltips, which a touch screen never shows. A pick, the overlay or
  Escape closes it, and focus returns to the menu button; while open, Tab stays
  inside it, and widening past 768px closes it. It renders after the app's
  content, so an app's own `fixed` bar at z-index 50 sits under it. With `hideTopBar`
  there is nowhere for the button, so the rail stays at every width. An app
  gets this by upgrading the SDK and republishing; nothing to change in its
  code.

### Added

- **`NavSheet`** (from `@oxy-hq/sdk/shell`) — that drawer as a presentational
  component, taking `ShellRail`'s `groups` / `footerItems` / `bottom` plus a
  `title`, for a host composing the shell pieces itself.

## [2.22.0] - 2026-10-28

### Added

- **The dock warms the app you are about to open.** `<OxyShell>` installs a
  Speculation Rules prefetch for the rail's other apps on hover (Chrome and
  Edge), so switching apps from inside one skips the page's round trip. A
  prefetch, never a prerender: nothing runs for a hover. `useDockPrefetch` is
  the hook behind it.

### Changed

- **`<NotificationsBell>` opens another app's notification, or a platform
  notice, in this tab**, as Oxygen's home page and dock now do, instead of a
  named tab per app (`oxy-app-<org>-<app>`). An absolute URL still opens in a
  new tab. Marking an item read is a `keepalive` request, so leaving the page
  does not cancel it.

### Removed

- **Source for Data Apps (`.app.yml`), which the server no longer serves.**
  Oxygen removed Data Apps (every `/apps/*` route now answers 404), and this
  deletes the client code that called them: `OxyClient` (`listApps`,
  `getAppData`, `runApp`, `getDisplays`, `getFile`, `getFileUrl`,
  `getTableData`), `OxySDK` (`loadFile`, `loadFiles`, `loadAppData`),
  `OxyProvider`, the Parquet reader, the Data App types (`AppItem`,
  `AppDataResponse`, `GetDisplaysResponse`, `DataContainer`, …) and the
  `react-vite-demo` example. **No published API changes:** none of this was
  reachable from any entry point since v2 (see 2.0.0 — the v1 `OxyClient` /
  `OxySDK` stack was dropped from the exports), so the built package is
  unchanged and this is not a major release.

## [2.21.0] - 2026-10-17

### Added

- **`oxy-app.json` has one definition, and it is this package's.** The
  `OxyAppManifest` type now carries every field the server honours —
  `rateLimitPerMinute`, `fetch.maxResponseBytes`, the per-function `storage`
  capability, `description`, `icon`, `art`, `status`, `build`, `environments`
  and `$schema` — each documented with what the platform does with it, and a
  JSON Schema is generated from it: `schema/oxy-app.schema.json`, shipped in
  the package (`https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json` for
  a manifest's `$schema`) and pinned by a test that regenerates it.
- **`@oxy-hq/sdk/manifest`** — a pure-TypeScript entry (no React, no DOM)
  with the manifest types, the schema (`OXY_APP_SCHEMA`), the two server name
  rules (`isValidSlug`, `isValidFunctionName`, their patterns and sentences),
  `validateOxyAppManifest(raw)` — a dependency-free validator over the schema
  plus the rules it cannot say, returning `{ level, path, message }` issues in
  one wording — and the notification catalogue's checks. `oxyc validate`,
  `oxyc publish`, `@oxy-hq/vite-plugin` and this SDK's own loader all run it;
  the two packages that carried byte copies of `notification-catalog.ts` now
  import it.

### Changed

- The loader (`loadCustomAppManifest`) validates against the schema instead of
  its own hand-written checks. The same manifests are refused, in the same
  sentences (`schemaVersion must be 2 …`, `` `slug` is required … ``,
  `function "x" must enable at least one of route/schedule/airwayStep/webhook`);
  a field the schema does not know is now logged as a warning rather than
  silently dropped, and the resolved `manifest` keeps every field the file
  holds.

## [2.20.0] - 2026-10-17

### Added

- **`ctx.platform.facts()` tells a customer's own use from ours, and prices
  each account's LLM usage.** For an internal app (the accounts view staff
  read); all four fields are optional, since a server older than this release
  sends none of them.
  - Per app, `customerLastActiveAt`: the latest production view by a non-staff
    person in the last 180 days. `lastActiveAt` is unchanged and still counts
    staff's opens, so a customer's app we keep opening no longer looks used.
  - Per app, `people7d`: distinct non-staff people with a production view in
    the rolling seven days `last7d` covers.
  - Per window (`weekly[]` and `last7d`, `OxyPlatformWeek`), `llmCostUsd`: the
    dollar cost of the window's `agentRuns` and every run they delegated to,
    priced exactly as Admin → Metrics prices it. Platform runs and staff
    previews and dry runs are left out, as they are from `agentRuns`.
    `llmUnpricedRuns` counts the runs on a model with no price, which add 0.

## [2.19.0] - 2026-10-16

### Added

- **A notification catalogue in `oxy-app.json`.** The app-level
  `notifications` block labels, groups and ranks the kinds an app's functions
  send — the tree people mute in (a whole app, a category, one kind) and the
  one a phone groups by. `categories` are paths of up to 3 segments
  (`operations/food-safety`; a declared path's parents are implied); each kind
  in `kinds` has a `category` (default `general`), a `label`, a `level`
  (`passive` | `active` | `time_sensitive` | `critical`; a function's
  `"critical": true` still makes it critical) and up to 3 `actions`
  (`{ id, label ≤ 24, function?, destructive? }` — with `function`, a tap calls
  that function as the person, then marks it read). Which kinds a function may
  send is still its own `notify.kinds`. Types: `OxyAppNotificationsManifest`,
  `OxyAppNotificationCategory`, `OxyAppNotificationKind`,
  `OxyAppNotificationAction`, `NotificationLevel`, and
  `OxyAppManifest.notifications`.
- **`ctx.notify({ …, thread })`** — group notifications about one thing
  (`store:${id}`, ≤ 128 characters). Inboxes fold a run of one thread into its
  newest; only a hash is stored, and items carry it as `thread`. The test
  context refuses a blank, over-long or non-string `thread` with the server's
  `InvalidNotifyPayload` wording.
- **`NotificationItem` gains `category` (`{ path, labels }`), `kind_label`,
  `level`, `thread`, `actions` and `url`**, all optional — a server older than
  the catalogue sends none of them and everything renders as before.
  `NotificationsPage.next_before` is the cursor for the next page.
- **`useNotifications({ category, kind, thread })`** narrows the list, the
  polled count and "Mark all read". `category` covers its subtree and names
  this app, so it needs the app id oxy injects. On a server older than the
  catalogue the hook filters the page itself and marks the listed items one
  by one rather than send a `read-all` that server would not narrow.
- **`<NotificationsBell />` shows the catalogue.** Each row names its category
  under the title (nothing for `general`), marks `critical` and
  `time_sensitive` notifications with a leading edge — themeable with
  `--oxy-notifications-critical` and `--oxy-notifications-time-sensitive` — and
  folds a run of one thread into its newest with "+N more"; opening it reads
  the whole run. `collapseThreads(items)` is the same folding for an app that
  draws its own.
- **`@oxy-hq/sdk/testing`:** `createTestContext` refuses a manifest whose
  catalogue publish would refuse, listing every problem;
  `createTestInbox({ manifest })` gives received items the category, label,
  level and actions the catalogue gives them, hashes `thread` as the server
  does (`threadKey`), and answers the `source`, `category`, `kind`, `thread`
  and `before` filters. `checkNotifications(manifest)` and
  `resolveNotificationKind(manifest, kind)` are exported for an app's own
  tests.

`oxyc validate`, `oxyc publish` (before the build) and `@oxy-hq/vite-plugin`
(`pnpm dev` and `pnpm build`) check the catalogue with the same rules as the
server and list every problem at once.

## [2.18.0] - 2026-10-14

### Added

- **`ctx.notify` — tell people in this org something.** A function names
  people (`{ userId }`, `{ role, location? }`, `{ team }`), a declared `kind`,
  a `title`, and optionally a `body`, a `link` inside the app, `email`, `slack`
  and an `idempotencyKey`. Each person gets an inbox row (frontline crew
  included), then push, email or Slack where they can be reached. Needs
  `"notify": { "kinds": { … } }` on the function in `oxy-app.json` and an OLTP
  store for the org. Kinds starting with `oxy.` are Oxygen's own and refused
  (`NotifyKindReserved`). `NotifyInput`, `NotifyResult` and `NotifyTarget` are
  exported from the root, and `createTestContext` enforces the capability and
  the declared kinds in the host's words and records each send on
  `state.notifications`.
- **`useNotifications()` and `<NotificationsBell />` — the inbox inside the
  app.** A custom app opens in its own tab, so people who live in one app
  (crew on a kiosk, a single-app user) never saw Oxygen's bell. Drop
  `<NotificationsBell />` into the app's header: a bell with an unread badge
  that opens this app's notifications, newest first, with "Mark all read". A
  tap on one of this app's opens it in place — `onNavigate(link)` for the
  app's router, else the page navigates to the link under the app's base path;
  another app's or one of Oxygen's own opens where Oxygen would, in a new tab.
  It polls the unread count every 30 s while the tab is visible and fetches
  the list only when opened. `scope: "all"` shows the whole inbox. Inline
  styles only; theme it with `--oxy-notifications-*` CSS variables (it also
  follows `<OxyShell>`'s tokens and Tailwind v4 `--color-*` ones).
  `useNotifications()` is the same data for an app that draws its own. On a
  staging or `dev-*` host it is read-only (`readOnly: true`): read state is
  shared with production, so the bell shows no "Mark all read" and a tap does
  not mark read.

  **Renders nothing where there is no inbox:** a server without
  `/api/notifications`, a page nobody is signed in to, or `pnpm dev` (which
  has no app id; use `scope: "all"` there). It needs a server that takes
  `app_id` on the inbox routes; on one that ignores it, the app's inbox hides
  itself rather than show, or mark read, other apps' notifications.
- **`createTestInbox()` in `@oxy-hq/sdk/testing`** — a `fetch` that answers the
  inbox routes from items a test holds, for `<OxyAppProvider fetcher={…}>`.
  `inbox.receive(state.notifications[0].input)` shows what a function test
  sent with `ctx.notify`.

## [2.17.0] - 2026-10-03

### Changed

- **`useQuery` sends its `params` to the server instead of writing them into
  the SQL.** The hook used to substitute `{{ params.X | sqlquote }}` itself,
  doubling `'` and nothing else. That is DuckDB's and Postgres's whole rule and
  nobody else's: ClickHouse, MySQL, Snowflake and Redshift also read a
  backslash as an escape, and BigQuery reads a backslash and does not read `''`
  as a quote. So a value holding a backslash — `C:\`, or `x\'` — broke the
  query on those engines, or changed what it asked. The browser cannot know
  which engine a project's database is; the server can. The request now carries
  the template untouched plus `params: { … }` for the names it uses, and the
  server writes each string by the rule of the engine that will read it.
  `{{ params.X }}` with no filter is still written unquoted, a number or
  boolean as itself, and a missing or `null` param as `NULL` (a `NaN` or
  infinite number is now `NULL` too, where it used to be written as the word).

  **Needs a server that accepts `params` on `POST /api/projects/{id}/query`.**
  An older one answers `400 invalid request body: unknown field "params"` to a
  query that uses a placeholder; a query with no placeholder sends the same
  request it always did. Not a security boundary before or after: that endpoint
  runs any read-only SQL a signed-in member sends.

## [2.16.0] - 2026-09-23

### Added

- **`@oxy-hq/sdk/testing` — a typed test context for Oxy Functions.**
  `createTestContext(manifest, { function, databases })` returns an
  `OxyFunctionContext` backed by in-memory stores that refuses exactly where the
  host refuses, in the host's own words: the manifest's capability gates
  (`secrets.write`, `email.send`, `org.read`, `storage.read` / `storage.write`,
  `oltp`, `airhouse`), the `destinations` allowlist and the customer-warehouse
  rule, `ctx.tx` on a non-Postgres database, `warehouse.upsert` where
  `ON CONFLICT` does not parse, `ctx.fetch`'s SSRF allowlist and byte cap. Every
  refusal is quoted from the host's source and held there by a Rust drift test,
  so a reworded refusal reaches an app's tests on its next SDK bump rather than
  never. Reads answer from a byte copy of the platform's shape zoo, so a
  `UInt64` above `i64::MAX` arrives as a string in a test as it does in
  production; `t.state.<store>.zoo()`, `.table(name, columns)` and `.raw(name,
  rows)` seed rows, the last marked `source: "author"` on every read of it.
  `t.calls`, `t.ops()`, `t.callsTo(op)` and `t.override(op, impl)` are typed by
  the host's closed `HostOp` list, and `t.run(fn)` evaluates a handler with the
  isolate's absent globals (`Buffer`, `TextEncoder`, `process`, …) removed.
  What it is not: a server, an engine, or a replacement for `oxyc checks run` —
  a test that passes here and fails there has found the host's words drifting
  from its behaviour, which is a platform bug to file. Spec and open questions:
  `internal-docs/sdk-testing-context.md`.

## [2.15.0] - 2026-09-23

### Added

- **`OxyFunctionRequest` declares `method` and `headers`.** The host has always
  sent `{ method, headers, body }` (`req_json` in `runtime.rs`); the type named
  only `body`, so the webhook-verification example in the functions guide —
  `req.headers["x-hub-signature-256"]` — was a type error against the SDK that
  documents it. Header names are keyed lower-case, a header sent twice collapses
  to its first value, and a scheduled or Airway run reports `POST` with `{}`.
  Nothing about the runtime value changed.

## [2.14.0] - 2026-09-23

### Added

- **`ctx.crypto` is declared on `OxyFunctionContext`.** The host has bound
  `ctx.crypto.hmac`, `ctx.crypto.verifyHmac` and `ctx.crypto.timingSafeEqual`
  since they shipped, and the authoring guide documents them, but the context
  type never declared them — so the documented webhook-verification call was a
  type error for a thing that works. `OxyCryptoApi`, `OxyHmacInput` and
  `OxyVerifyHmacInput` are exported. Typed from `__buildCtx` in `runtime.rs`:
  synchronous (no `await`);
  `algorithm` defaults to `"sha256"`, `encoding` to `"hex"`; `key` and `data`
  are required strings; `signature` and both sides of `timingSafeEqual` accept
  an absent value, because the host answers `false` for one rather than
  throwing.

### Fixed

- **`ctx.query` resolves to `{ rows, truncated }`, as the host has always sent
  it.** The type said `OxyFunctionRow[]`. A function that indexed or iterated
  the result compiled and failed on its first call (`rows.map is not a
  function`; `.length` reading `undefined`), while the docs, the scaffold and
  the platform canary already destructure `rows`. If you typed against the old
  shape, change `const rows = await ctx.query(sql)` to
  `const { rows } = await ctx.query(sql)` — the runtime value is unchanged.

## [2.13.1] - 2026-09-22

### Fixed

- **An aborted request no longer strands a fetching hook on `loading: true`.**
  `useSemanticQuery`, every metric-tree hook, `useWorldModelGraph`,
  `useWorldModelInstances`, `useMeasureBreakdown`, `useProcedureRun` and
  `useAgentRun` returned early on any `AbortError`, matched by name. Their own
  teardown is already marked (the per-run `cancelled` flag, or the run's
  `AbortController`), so the name check only ever fired for an abort the hook
  did NOT cause — a fetcher carrying its own timeout, a dev-proxy socket drop,
  a navigation. That read as `loading: true, error: null` for the rest of the
  page's life: skeletons with nothing to explain them, and a runner parked on
  `"running"`. Silence is now decided by whether we tore the run down; every
  other abort is reported as the failure it is.
- **An abort surfaced this way no longer reads as "The user aborted a
  request."** — no user did. It is reported as "request was interrupted (the
  connection dropped or timed out)", carrying the original as `cause`. The
  error's name is used only to word an error already being reported; whether
  to report at all is still decided by the hook's own teardown.
- **`useAgentRun`'s reconnect budget counts consecutive dead windows, not
  windows.** Five was the ceiling for the whole run, so a long stream that
  reconnects — a `fetcher` with its own request timeout, a proxy recycling the
  connection, an HTTP/2 `GOAWAY` — was reported `failed` after five windows
  even though every one of them delivered events. A window that made progress
  now starts the count over, whether it ended in a transport error or a
  graceful EOF; five windows that deliver nothing still give up. The server
  resumes from `Last-Event-ID`, so a reconnect never re-delivers and progress
  always means new events.
- **`refetch()` works on the metric-tree and world-model hooks.** The nonce it
  bumps was missing from the effect's dependency list, so every metric-tree
  hook plus `useWorldModelGraph` / `useWorldModelInstances` ignored it
  entirely — a panel whose read had failed could not be retried at all.
  `useMeasureBreakdown` has no `refetch` of its own: it is re-run by changing
  its `entityId` / `keyValue` / `measure`.

## [2.13.0] - 2026-09-09

### Added

- **`signOut(fetcher, links.login)` in `@oxy-hq/sdk/shell`, and `links.login`
  on the shell context.** An app ends its viewer's platform session with
  `GET /api/logout` and leaves for the product's login page with itself as
  `return_to`. On an enrolled tablet that page is "Who's on shift?", and the
  next PIN comes back into the app as that person; elsewhere it is the
  ordinary sign-in. `signOutUrl` is the pure half. A refused logout throws so
  the caller never navigates away from a session that still exists. The
  server names the login page because a custom-app subdomain cannot derive
  the product host.

## [2.12.0] - 2026-09-07

### Added

- **`ctx.org.places()` and `ctx.org.assignments()`.** The org's locations
  (hierarchy via `parent_id` and a tenant-named `kind`, lifecycle `status`,
  `timezone`, and `external_ids` — what Toast, a camera console or payroll
  call the place) and the roster (who holds which position where, and under
  whom). Same `org.read` capability as `people()`, same fail-closed refusal
  without it; `assignments()` is scoped to people who can reach the app, as
  `people()` is. Types `OxyOrgPlace` and `OxyOrgAssignment`. This is the
  platform half of "where you work decides what you see": an app reads the
  roster instead of keeping its own `staff` and `locations` tables. Design:
  `internal-docs/operating-graph.md`.
- **`ctx.user.reach`.** Where the caller may act, decided by the platform from
  their assignments before the function runs: `{ everywhere, via, locations }`
  — a system invocation, an app admin or a holder of an org-wide position
  everywhere; an assigned person exactly their places; an unassigned member
  everywhere and an unassigned frontline worker nowhere. A lookup failure
  lands on nowhere.
- **`@oxy-hq/sdk/ops`.** `reachOf`, `reaches`, `requireReach` (403
  `OutOfReach`), `predicate` (the WHERE fragment), `isAdmin`, `adminOnly` —
  Store Ops's `functions/access.ts` lifted into the SDK minus its roster SQL.
  Tighten, never widen: an absent reach reads as nowhere, including for a
  system invocation on a server without the graph.
- **Place in the semantic model.** A view's primary entity may declare
  `binding: { registry: locations, system: toast }`; with it,
  `useWorldModelInstances` items carry `location` (`{ id, name, kind,
  parent_id }`) when the key is mapped, `useWorldModelInstances(id, { scope:
  "reach" })` keeps only the instances the viewer reaches (`withheld` counts
  the rest), and `ctx.semantic.query({ ..., scope: "reach" })` — and the
  browser semantic query with `scope: "reach"` in its body — pins the query to
  the caller's places server-side. Toast order events now carry `location_id`
  / `location_name` when the restaurant GUID is mapped.
- **`useIdentity()`** (`@oxy-hq/sdk/shell`): the viewer in one shape on every
  server version — `id`, `name`, `email` (`null` for the crew), `picture`,
  `kind`, display-only `reach`. The shell context's `user` gains `id`, `kind`
  and `reach` on servers that carry the graph.

## [2.11.0] - 2026-09-06

Scenario forecasting: three metric-tree hooks for the what-if canvas, split by
what each one costs — documented in `docs/build/sdk/hooks.mdx` under "Scenario
forecasting".

### Added

- **`useBaseline` / `client.metricTree.getBaseline`** — values every node
  reachable from a set of levers over a window, and fits a coefficient for any
  driver edge whose `.view.yml` declares none.
- **`useProjection` / `client.metricTree.getProjection`** — bucketed history
  for the levers and everything downstream, plus the forecaster's forward
  curve. Returns the baseline curve only: the scenario's second curve is
  composed client-side from this and a `usePredict` result, so editing a lever
  never costs a query. A series carrying `refusal` (most often too little
  history to fit) is a stated absence, never a flat forward line.
- **`usePredict` / `predict()` take an options bag (`{ values, coefficients }`)**
  — feed `useBaseline`'s `values` and `fitted` straight through so an
  undeclared driver edge propagates instead of silently dropping its
  downstream measures from `impacts`. Existing calls with no second argument
  are unaffected.
- New exported types: `BaselineInstance`, `BaselineRequest`, `BaselineResponse`,
  `FittedDriver`, `ForecastPoint`, `HistoryPoint`, `MeasureProjection`,
  `MeasureValues`, `PredictOptions`, `ProjectionGranularity`,
  `ProjectionRequest`, `ProjectionResponse`, `UnvaluedNode`, `UsePredictOpts`.
- `DriverForm` grows five shapes (`quadratic`, `cubic`, `sqrt`, `inverse`,
  `linear-log-quadratic`) to match what airlayer can already emit — a tree
  carrying one of these edges no longer types as a value the union couldn't
  hold.
- `MetricEdge.operator` (`"add" | "sub" | "mul" | "div"`, absent means `add`)
  and `MetricEdge.form_declared` (whether the shape was authored in YAML or
  inferred by the fit).
- `MetricNode.drillable` — whether a measure can be drilled into; serialized
  rather than re-derived, since `measure_type` alone misses eligible
  composites.
- `MetricTree.warnings` — refusals raised while building the tree, e.g. a
  driver declaring both `coefficient:` and `coefficients:`.
- **`useTrack` events name their app.** `useOxyApp()` now returns `appId`
  (from `window.__OXY_APP__`), and each tracked event carries it as `app_id`.
  The events endpoint is keyed by workspace, and a workspace can publish
  several apps — without this the server attributed an event to *an* app in
  the workspace. Bundles built against an older SDK keep working; the server
  falls back to the old lookup when `app_id` is absent.

### Changed

- **`ctx.user.email` is `string | null`.** It was typed `string`; the runtime
  has sent `null` for a frontline worker — a crew member enrolled by PIN, with
  no mailbox — since workers existed. Source-breaking for an app that hands
  `ctx.user.email` to something that wants a `string` (the `functions`
  scaffold's `notify.ts` did, and now answers 400 when there is nobody to send
  to). That null is the one field that tells the crew from the office inside a
  function.

## [2.10.0] - 2026-09-03

### Changed

- **A function's HTTP status now reaches the caller.** `readFunctionSseStream`
  throws a `FunctionStatusError` when a function returns a non-2xx, carrying
  `status`, the parsed `body`, and the logs captured before it. Previously the
  server hardcoded `{"status": 200}` in the terminal frame and dropped the
  isolate's status, so a handler answering 400, 403 or 409 resolved as an
  ordinary success and every caller had to infer rejection from the body's
  shape.

  **This is a behaviour change.** An app that reads `{ error: … }` out of a
  resolved value will now see a rejection instead:

  ```ts
  try {
    await invoke({ pathId });
  } catch (e) {
    const err = e as FunctionError;
    if (err.status === 409) { /* everyone already holds it */ }
  }
  ```

  Two things keep it from breaking apps that do not return a non-2xx: a missing
  or non-numeric status is treated as success, so an app talking to a server
  that predates this is unaffected; and the check is a 2xx range rather than
  `!== 200`, so 201 and 204 stay successes.

  Requires an oxy server carrying the matching change; against an older one the
  behaviour is unchanged.

### Added

- `FunctionError`, `FunctionLog` and `FunctionResult` are exported. They were
  the SSE reader's types and had never been re-exported, so a consumer could not
  name the value a `catch` gives them.

## [2.9.1] - 2026-08-21

Review corrections to 2.9.0, which had not shipped yet — folded in rather than
left as a footnote, since the wording they fix is the contract itself.

- **`kind: "system"` also covers a manual *Run now***, not just cron and Airway.
  2.9.0 documented `"system"` as "no human behind it"; an operator clicking Run
  now in the admin console is a human, and the job trigger deliberately routes it
  down the same owner-identity path. The field now means *no caller to attribute
  this to* — which is the honest claim, and the one a function can act on. The
  triggering operator is still unreachable; carrying them through the task
  payload is a follow-up on the server.
- `ctx.user.teams` is now sorted **case-insensitively**. Byte order put every
  lowercase team name after every capitalised one, which renders as unsorted.

## [2.9.0] - 2026-08-21

Fills out `ctx.user`. A function could previously answer "who is calling" with an
id, an email, and a per-app role — enough to gate on, not enough to greet, label,
or explain with, and with no way to tell a human apart from a cron tick.

- **`ctx.user.name` / `ctx.user.picture`** — display identity, server-side. The
  workaround was passing them up from the client, which is forgeable and so
  defeats the point of reading identity from `ctx` at all.
- **`ctx.user.orgRole`** (`"owner" | "admin" | "member"`) — their standing in the
  owning org, absent for a staff break-glass visitor. Informational by design:
  gate on `appRole`, use this to explain ("ask your org admin to connect a
  warehouse"), label, or route.
- **`ctx.user.teams`** — the org teams they belong to, name-sorted and scoped to
  this app's org. Descriptive: a team confers something on an app only through an
  app team grant, which `appRole` already folds in.
- **`ctx.user.kind`** (`"user" | "system"`) — the supported way to tell a routed
  invocation from a background one. `"system"` means *no caller to attribute
  this to*, which also covers an operator's manual **Run now**: those run under
  the org owner's id with every caller field absent, so they still read
  `appRole: "admin"`, and the triggering operator is not reachable. The
  email-sniffing check this replaces (`endsWith("@system.oxy")`) was never a
  contract.
- **Fixed: `ctx.user.orgId` was `undefined`.** The host serialized the field as
  `org_id` while these types and every doc said `orgId` — so a tenant filter
  written against it compared against nothing. The host now sends `orgId`, and
  the runtime keeps `org_id` populated so functions written against the shipped
  behaviour keep working. **If a function of yours filters SQL on the org id,
  re-read that line.**
- New exported types: `OxyOrgTeam`, `OxyIdentityKind`.
- `teams` and `kind` are **optional** on the type. A server older than this
  release doesn't send them, and a required type would have promised otherwise:
  `ctx.user.teams.some(...)` throws there, and `kind === "system"` silently reads
  `false` — routing a cron tick down the human branch, the exact bug `kind`
  exists to prevent. There is no safe inference for an absent `kind` in either
  direction; mark the schedule's `input` if you must support one.

Full contract, including what is deliberately withheld from apps:
`internal-docs/custom-apps-user-identity.md`.

## [2.8.0] - 2026-08-02

Makes email attachments actually reachable from an Oxy Function. `content` was
documented as base64, but the Functions isolate is bare `deno_core` — no
`deno_web`, and a V8 predating `Uint8Array.prototype.toBase64` — so `btoa` was
`undefined` and there was no way to produce that base64 at all.

- **`bytesToBase64` / `base64ToBytes`** — new named exports for binary. Plain
  bundled JS, so they behave identically in the isolate, in Node/vitest, and in
  a browser; and because they take bytes, `bytesToBase64(u8)` cannot be misread
  the way `btoa(u8)` is (the spec stringifies it to `"37,80,68,70"` and encodes
  that text instead of your file).
- `btoa` / `atob` now exist in the isolate (polyfilled in the runtime
  bootstrap), following WHATWG semantics so a helper unit-tested under Node
  behaves the same there. Passing bytes to `btoa` throws and points at
  `bytesToBase64`. These globals are declared for the functions tsconfig only —
  deliberately **not** added to this package's browser type graph, where `btoa`
  is the real DOM one with no polyfill behind it.
- `EmailAttachment.encoding` (`"base64"` default, or `"utf8"`) — attach
  generated text (CSV/JSON/HTML) with no encoder and byte-exact for non-ASCII.
- `ctx.fetch` takes `encoding: "base64"` (`OxyFetchInit`) and echoes it on the
  result. The default UTF-8 decode is lossy, so fetching a binary to attach
  previously returned a silently corrupt file.

## [2.7.0] - 2026-07-27

Completes the anomaly inbox for standalone custom apps. `scan()` and
`explain()` were already in the SDK but 404'd against `/external/api` —
the surface a bundle served from its own origin has to use — because only
the read half of the inbox was mounted there. Both verbs are now exposed,
so `client.anomalies.*` behaves the same from a custom app as from the IDE.

### Added

- **`explain(id, { refresh: true })`** — busts the cached `ExplainResult`
  and recomputes. The server already accepted `?refresh=true`; the option
  was simply unreachable from the SDK.
- **`ScanResponse.pending` and `ScanResponse.failures`** — a scan that
  outruns the 55 s synchronous window (or is debounced within 60 s of a
  previous one) returns `pending: true` with **all counts zeroed**. Those
  zeros are not a "nothing found" result; check `pending` before reading
  them, then refetch with `list()`. `failures` carries per-monitor errors.
- **`Anomaly.dimension_key` / `Anomaly.filters`** — the segment a row
  belongs to. Already on the wire, previously absent from the type, so
  per-segment anomalies couldn't be told apart from chain-wide ones.
- **Exported types** `AnomalyFilter`, `ScanFailure`, `ExplainOptions`.

### Changed

- **`Anomaly.filters`, `ScanResponse.pending` and `ScanResponse.failures`
  are required-nullable, not optional.** The server serializes all three
  unconditionally, so the previous `?` overstated absence — `filters` is
  `null` for a chain-wide monitor and `failures` is `[]` on a clean scan,
  but neither is ever missing. Reading code gets strictly better types;
  code that **constructs** these objects (test fixtures, mocks) must now
  supply the fields.

## [2.6.0] - 2026-07-23

Adds a custom-app **asset store**, **email attachments**, and a
**per-app role** on `ctx.user` — three pieces of one story: an app can
now accept a file, keep it, show it back, email it, and restrict who
sees any of that.

### Added

- **`ctx.storage`** — the app's asset store, covering both kinds of file
  an app produces, in one per-app silo:
  - **Uploaded**: `getUploadUrl()` mints a presigned PUT and the browser
    uploads **straight to S3**, so uploads aren't bounded by the request
    body limit and the bytes never pass through the function or oxy.
  - **Generated**: `put(pathname, body, { encoding: "base64" })` writes a
    file the function itself produced — binary (PDF, PNG, Parquet) is
    first-class, not text-only.

    Full surface: `getUploadUrl`, `getDownloadUrl`, `put`, `get`, `head`,
    `list`, `delete`, `copy`. Gated by new fail-closed
    `storage: { read, write }` capabilities in `oxy-app.json`.

    Notable defaults, and why: **`allowOverwrite` is false** (silently
    clobbering an asset is worse than an error — enforced atomically via
    an S3 conditional write, not a racy check-then-put); **`list` is
    cursor-paginated** (a silo with 100k assets must not become one
    unbounded walk); **download links can live up to 7 days**, SigV4's own
    limit, because a link emailed to a human outlives a 15-minute upload
    window. Every asset is private — reads are always presigned and
    time-boxed; there is no public-access mode by design.

    Keys are confined to the calling app: another app's key is rejected on
    every operation, not just on read.

- **Email attachments** — `ctx.email.send({ attachments: [...] })` with
  `filename`, base64 `content`, optional `contentType`, and `inline` +
  `contentId` for `cid:`-referenced images. Max 20 per send and 10 MiB
  decoded in total; past that, store the file with `ctx.storage` and email
  a presigned link instead (which is the better shape anyway, since the
  file is usually retained regardless).

- **`ctx.user.appRole`** — `"admin"`, `"member"`, or absent, derived
  server-side from per-app membership (with org-owner / Oxy-staff
  break-glass). This is what a privileged in-app surface should gate on:
  unlike a query param or a client-side flag, the client cannot forge it.
  Deliberately *not* the org role — an app admin administers one app
  without holding org-Admin, which also carries billing and member
  management.

## [2.5.0] - 2026-07-22

Universalizes the custom-app shell: a bundle served from any origin can
now drive the wired shell (shell-context, Ask Oxygen) and theme its chrome
to match the host app.

### Added

- **`backendUrl` prop on `OxyAppProvider`** — the SDK resolves its relative
  `/api/*` requests against this origin, so a cross-origin bundle reaches
  the Oxy backend without a same-origin proxy. Opt-in: when unset, the
  fetcher is unchanged.
- **`chromeBackground` / `chromeForeground` props on `OxyShell`** — theme
  the rail + top bar + AskDock by overriding the host tokens
  (`--sidebar-background`, `--foreground`, `--muted-foreground`), which
  every shell sub-scope re-derives from.
- **AskDock history panel** — shows title + relative time ordered by
  `created_at`, keeps and highlights the active chat, and adds a search box
  and a "Show more" control.

### Removed

- **The built-in Settings rail item.** The shell adds no built-in rail
  entries — a bundle that wants a Settings link supplies it via
  `railBottom`.

### Fixed

- `dev` / `build:watch` now emit `dist/shell.css` too (tsdown `onSuccess`
  plus a CSS watcher), so live CSS edits are reflected instead of going
  stale until the next full `build`.

## [2.4.0] - 2026-07-21

Ships the Oxygen workspace shell — the same 48px icon rail + universal
top bar the main web-app renders — as a reusable subpath export, so a
custom app reads as part of the HQ. The web-app consumes these exact
components (oxygen-internal `internal-docs/2026-07-06-sdk-shell-universalization-design.md`).

### Added

- `@oxy-hq/sdk/shell` entry point: wired `OxyShell` + `useShellContext`
  (bootstraps from the new bundle-gated `GET /api/projects/:id/shell-context`;
  degrades to chrome-less rendering on older servers), and presentational
  `ShellRail`/`RailItem`, `TopBar`, `Breadcrumb`, `SystemIndicator`,
  `WorkspaceClock`, `WorkspaceTile`, `ShellTooltip`, `OxyMark`,
  `OxygenFactoryMark`, `workspaceLogoUrl`.
- `@oxy-hq/sdk/shell.css` — namespaced (`oxy-shell-*`) stylesheet; no
  Tailwind required. Follows host design tokens when present, falls back
  to the Oxygen defaults; dark mode via a `.dark` ancestor class.
- `useOxyApp()` is now exported (low-level identity + fetcher access
  without requiring the manifest to be ready).
- **Reasoning trace in the Ask dock** — the same trace the main web-app
  renders: header meta (LLM calls · total time · steps), step rows with
  status, indented tool rows with input previews + durations, streamed
  thinking text, and query rows with row counts. Expanded while
  streaming, auto-collapses when the run settles. Exported standalone as
  `ReasoningTrace` + `buildTraceSteps`/`aggregateLlmStats`.
- **Interactive charts in the Ask dock** — `AnswerChart` renders
  `chart_rendered` blocks with **ECharts** (the same library the main
  web-app uses) when the host app has it installed: axis/item tooltips,
  hover, legend, resize. `echarts` is an optional peer (dynamic import);
  bundles without it fall back to the dependency-free SVG render. Table
  type always uses the SVG/HTML table.
- **Native charts in the Ask dock** — `chart_rendered` blocks from the
  analytics pipeline render as dependency-free SVG (bar/line/pie, table
  fallback) via the exported `AnswerChart`; the duplicate QUERY artifact
  block is gone from the dock (the trace + chart carry that info).
- **API-backed chat history** — the Ask dock's History now lists the
  viewer's persistent threads from the bundle-gated
  `GET /api/projects/:id/threads`, merged with the richer in-session
  conversations. Opening a server thread fetches
  `GET /api/projects/:id/threads/:tid` and rebuilds the transcript (trace,
  charts, answer) by replaying the thread's persisted run events through
  the same processor the live stream uses; follow-ups resume the thread.
  Exported: `useThreadHistory`, `fetchThreadTranscript`. Requires an oxy
  server that stamps `user_id` on bundle threads (same release); older
  servers fall back to session-only history.
- **New chat + history in the Ask dock header** — a `＋` action starts a
  fresh conversation (new server thread) and a clock action opens
  session-scoped history: chats started in this dock session, each
  restorable (follow-ups resume its server thread via the SSE resume).
  Bundles have no list-threads endpoint, so history is per-session, not
  persisted across reloads.
- **Ask Oxygen dock** in the shell: when the app manifest binds an agent
  (`ask.agent`), the top bar shows the Ask Oxygen button (⌘K/Ctrl+K
  toggles) and a right-side chat dock opens as a flex sibling —
  compacting the app, not covering it. Multi-turn over `useAgentRun`
  (follow-ups reuse the thread), suggested-question chips from
  `ask.suggestedQuestions`, streamed answers with SQL artifacts, and an
  "open in Oxygen" header link to the product Chat surface. Also
  exported standalone as `AskDock`.
- The shell rail's bottom cluster shows a **Settings** entry linking to
  the product's Unified Settings Dialog (via the SPA's new
  `?settings=<section>` deep link, returned as `links.settings` by
  shell-context; hidden on servers that don't send it).

### Fixed

- `OxyAnswer` / `OxyChat` follow the host's design tokens when rendered
  inside the shell scope (dark mode included) — hardcoded light-theme
  colors remain only as fallbacks for standalone bundles. The spinner's
  `oxy-spin` keyframes are now actually defined (injected once), so it
  spins.

### Changed

- New runtime dependency: `@radix-ui/react-tooltip` (rail tooltips).
- New optional peer: `react-dom` (only needed by the shell's tooltip
  portal; data-only consumers are unaffected).

## [2.3.0] - 2026-07-20

### Added

- **Customer-app email sending** (oxy-internal `feat/customer-app-email-send`) —
  Oxy Functions can send email via `ctx.email.send({ to, subject, html|text, ... })`,
  backed by AWS SES. The **platform controls the `from` address**; the function
  sets `replyTo` only. Gated by a new fail-closed `email: { send: true }`
  capability on `OxyAppFunctionManifest`.
- **`@oxy-hq/sdk/email`** — a new subpath export shipping
  `render(Component, props)` (preact-render-to-string) so functions can author
  email bodies as **preact** components and render them to HTML inside the
  Functions isolate. `preact` / `preact-render-to-string` are optional peer
  deps, so the main SDK bundle is unaffected. (React Email / react-dom can't run
  in the isolate: its node build needs `node:stream`, its browser build needs
  Web Streams the isolate lacks.)
- **`OxyFunctionContext`** — the server-side function `ctx` is now typed
  (`user`, `env`, `log`, `query`, `queryStream`, `fetch`, `warehouse`, `secrets`,
  `semantic`, `airway`, `email`), exported from the custom-app entry.

## [2.2.0] - 2026-07-09

Publishes the custom-app **Oxy Functions** platform and the **metric-tree /
anomalies** clients that accumulated on `main` since 2.1.0. All additive — no
public export was removed or renamed.

### Added

- **Oxy Functions** (oxy-internal #2521) — `useFunction(name)` for invoking
  server-side TypeScript handlers shipped in a bundle's `functions/` dir, and the
  `functions` block on the app manifest (`route`, `timeoutSeconds`, `cache`,
  `destinations`).
- **Scheduled functions + secret writes** (#2685) — `schedule` (cron) +
  `timezone`, `airwayStep`, and `secrets: { write }` on `OxyAppFunctionManifest`,
  each with manifest validation. A function can now run on a cron schedule and
  persist app-scoped secrets via `ctx.secrets.set`.
- **Metric tree + anomalies** (#2407) — `MetricTreeClient` and `AnomaliesClient`
  and their result types (metric nodes/edges, driver attribution, anomaly
  list/scan).
- **Result caching + function SSE** (#2634) — client-side query-result cache
  (dedup + TTL) and the streaming transport for function invocations.

## [2.1.0] - 2026-06-08

Adds engineer-tagged usage events to the SDK so a bundle can record
which features its users actually exercise. Pairs with the per-app
**Activity** tab in the Oxy admin UI (oxy-internal #2465 / §13 of
`internal-docs/customer-apps.md`).

### Added

- `useTrackEvent()` — fire-and-forget hook returning
  `(name: string, payload?: object) => void`. Batches every 1s and
  flushes on `pagehide` via `sendBeacon` so a click that fires
  immediately before a navigation isn't lost. Server-validated
  `event_name` regex (`^[a-z][a-z0-9-]{0,63}$`) + 4 KiB payload cap
  + 60-events-per-minute rate limit per (user, app).

  ```tsx
  import { useTrackEvent } from "@oxy-hq/sdk";

  const track = useTrackEvent();
  <button
    onClick={() => {
      track("export-clicked", { format: "csv", rowCount });
      doExport();
    }}
  >Export</button>
  ```

### Notes

- View events (page loads) are recorded automatically by the oxy
  backend on every HTML serve — no SDK code required. The SDK hook
  is only for engineer-tagged interactions inside the bundle.
- Dev-mode caveat: `sendBeacon` carries cookies but bypasses the
  `OxyAppProvider` fetcher wrapper, so the `OXY_TOKEN` bearer
  injected by the vite-plugin proxy in cross-origin `pnpm dev` is
  missing on these requests. Cookie-served prod is unaffected.

## [2.0.0] - 2026-05-29

Complete rewrite: `@oxy-hq/sdk` is now a **React-only, customer-app-only**
SDK. A bundle wraps its tree in `<OxyAppProvider>` and reads from its linked
oxy project through hooks; identity is resolved from `oxy-app.json` +
`window.__OXY_APP__` (injected by oxy at serve time), and requests are
authenticated by the session cookie (same-origin) or a bearer token
(cross-origin dev).

### Added

- `OxyAppProvider` — resolves app identity and provides it via context.
- Hooks: `useQuery` (inline SQL), `useSemanticQuery` (semantic layer),
  `useAgentRun` (agent chat over SSE), `useProcedureRun` (long-running
  procedures, beta).
- Drop-in components: `<OxyChat>` and `<OxyAnswer>` (markdown + SQL
  artifacts; URL-scheme allowlist guards against `javascript:` injection).
- `OxyApiError` structured error envelope.
- Pairs with `@oxy-hq/vite-plugin` (base path, manifest copy, dev shim) and
  `create-oxy-app` scaffolding.

### Removed (BREAKING)

- The entire v1 stack: `OxyClient` / `OxySDK` / `OxyProvider`, the
  Parquet/DuckDB-WASM reader, and postMessage-based auth. Apps now talk to
  `/api/projects/:id/*` exclusively.
- `listApps` / `getAppData` / `runApp` / `getDisplays` / `getFile` /
  `getFileUrl`.

## [0.1.0] - 2025-01-01

### Added

- Initial release of the Oxy TypeScript SDK
- Core `OxyClient` with methods for app data fetching
- Configuration management with environment variable support
- Parquet file reading with DuckDB-WASM integration
- `ParquetReader` class for SQL queries on Parquet data
- Helper functions for quick Parquet data access
- Full TypeScript type definitions
- Comprehensive examples for Node.js, React, and v0 integration
- Documentation and API reference

### Features

- `listApps()` - List all apps in a project
- `getAppData()` - Fetch app data with caching
- `runApp()` - Run app and get fresh data
- `getDisplays()` - Get display configurations
- `getFile()` - Fetch files from state directory
- `getFileUrl()` - Get direct file URLs
- Parquet reading and SQL querying capabilities
- Support for both CommonJS and ES modules
- Browser and Node.js compatibility