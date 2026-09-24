# @oxy-hq/sdk

React SDK for building **custom-app bundles** on the [Oxy](https://oxygen-hq.com)
platform. A bundle is a normal Vite + React app that reads from its linked oxy
project — raw SQL, the semantic model, agents, and procedures — through a
small set of hooks, plus a couple of drop-in components.

> **v2 is a complete rewrite.** The v1 stack (`OxyClient` / `OxySDK`, the
> Parquet/DuckDB-WASM reader, postMessage auth) is gone. Bundles now talk to
> `/api/projects/:id/*` exclusively. See `CHANGELOG.md`.

## Install

```bash
pnpm add @oxy-hq/sdk @oxy-hq/vite-plugin
```

`react` (^19) is a peer dependency. `@oxy-hq/vite-plugin` wires the served
base path, copies `oxy-app.json` into the build, and injects the dev identity
shim — drop it into `vite.config.ts`:

```ts
import oxyApp from "@oxy-hq/vite-plugin";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({ plugins: [react(), oxyApp()] });
```

## Quick start

Wrap your tree in `<OxyAppProvider>` (it resolves the app's identity), then
read data with hooks:

```tsx
import { OxyAppProvider, useQuery, OxyChat } from "@oxy-hq/sdk";

function Dashboard() {
  const { rows, loading, error } = useQuery({
    sql: "SELECT Store, SUM(Weekly_Sales) AS sales FROM oxymart GROUP BY 1 ORDER BY 2 DESC LIMIT 5"
  });
  if (loading) return <p>Loading…</p>;
  if (error) return <p>{error.message}</p>;
  return (
    <>
      <table>{rows.map((r) => <tr key={r.Store}><td>{r.Store}</td><td>{r.sales}</td></tr>)}</table>
      <OxyChat agentId="analytics" />
    </>
  );
}

export function App() {
  return (
    <OxyAppProvider fallback={<p>Loading…</p>}>
      <Dashboard />
    </OxyAppProvider>
  );
}
```

## Identity (`oxy-app.json`)

Every bundle ships an identity-only manifest at its project root (next to
`vite.config.ts`, **not** under `public/`):

```json
{ "schemaVersion": 2, "slug": "store-pulse", "orgSlug": "acme", "name": "Store Pulse" }
```

When oxy serves the bundle it injects the authoritative identity as
`window.__OXY_APP__`; `OxyAppProvider` reads injection first and the manifest
second. There is **no API key in the bundle** — requests are authorized by the
viewer's oxy session (same-origin cookie) or, in cross-origin local dev, a
bearer token the dev proxy attaches. A bundle can't read data its viewer
couldn't already read.

## API

| Export | What it does |
| --- | --- |
| `OxyAppProvider` | Resolves identity, provides it via context. `fallback` renders while loading; `errorFallback` gets a structured error report. |
| `useQuery({ sql })` | Inline SQL → rows. `SELECT`/`WITH` only, 10k-row cap. |
| `useSemanticQuery({ topic, dimensions, measures, … })` | Semantic-model query compiled by airlayer. |
| `useAgentRun({ agentId })` | `.ask(question)` starts an analytics agent run; streams events over SSE; `.cancel()`. |
| `useProcedureRun({ procedureId })` | Start a long-running procedure, poll, cancel (beta). |
| `useFunction(name)` | `.invoke(body?)` runs a server-side **Oxy Function** (`functions/<name>.ts`) on oxy's isolate runtime; returns its JSON `Response`. For work the browser shouldn't do — warehouse writes, ELT, external APIs. |
| `<OxyChat agentId="…" />` | Drop-in chat UI over `useAgentRun`. |
| `<OxyAnswer … />` | Renders markdown + SQL artifacts + thread link. URL schemes are allowlisted (rejects `javascript:` etc.). |
| `OxyApiError` | Structured `{ message, code? }` server-error envelope. |

### World Model & analysis hooks

The same airlayer analyses the IDE's **World Model** and **Metric Tree** run,
exposed as hooks so a bundle can do RCA, opportunity sizing, and driver
exploration itself. Each fetches when enabled and its input is present; pass
`null` for a request/id to keep a hook idle until the user makes a selection.

| Export | What it does |
| --- | --- |
| `useWorldModel()` | The World Model **node interface** — `world.metric(id)` returns a handle speaking `expand` / `drivers` / `explain` / `size` / `drill`. |
| `useWorldModelGraph()` | The raw entity/measure graph — entities, their measures, and how measures promote across the hierarchy (edges). |
| `useWorldModelInstances(entityId, { search?, limit? })` | Searchable listing of an entity's instances (primary key + display label). |
| `useMetricTree({ root? })` | The metric tree (measures + component/driver edges), or the subtree at `root`. |
| `useSensitivity(measureId)` | Ranked **drivers** of a measure — "what moves this?" |
| `usePredict(changes, { values?, coefficients? })` | **What-if**: propagate hypothetical `(measure, delta)` changes upward (pure tree walk, no warehouse). Feed it `useBaseline`'s `values` / `fitted` or undeclared driver edges propagate nothing. |
| `useBaseline(request)` | **Scenario levels**: current values for the levers and everything downstream, plus coefficients fitted for driver edges that declare none. |
| `useProjection(request)` | **Scenario forecasting**: bucketed history + forward curve (with prediction band) for the levers and everything downstream. |
| `useExplain(request)` | **RCA**: period-over-period root-cause decomposition. |
| `useOpportunity(request)` | Segment **opportunity sizing** — addressable upside vs a benchmark peer. |
| `useDistribution(request)` | Single-period distribution against an auto-derived prior baseline. |
| `useTimeDimensions()` | Valid time dimensions per view — the period axis for the ops above. |
| `useMeasureBreakdown(entityId, key, measure)` | Per-instance **driver tree** (SSE) — node values fill in as they resolve. |

```tsx
import { OxyAppProvider, useExplain, useOpportunity } from "@oxy-hq/sdk";

function RootCause() {
  // Pass `null` instead of the request object to defer until the user picks a period.
  const { data, loading, error } = useExplain({
    target: "financials.operating_profit",
    time_dimension: "financials.month",
    current_period: ["2025-09-01", "2025-09-30"],
    previous_period: ["2025-08-01", "2025-08-31"]
  });
  if (loading) return <p>Explaining…</p>;
  if (error) return <p>{error.message}</p>;
  return <p>Δ {data?.target_delta} — {((data?.coverage ?? 0) * 100).toFixed(0)}% explained</p>;
}

function Upside() {
  const { data } = useOpportunity({
    target: "orders.net_revenue",
    time_dimension: "orders.order_date",
    period: ["2025-04-01", "2025-06-30"]
  });
  return <>{data?.dimensions.map((d) => <p key={d.dimension}>{d.dimension}: +{d.total_upside}</p>)}</>;
}
```

Scenario forecasting is three hooks, split by what each one costs.
`useBaseline` and `useProjection` each fire one warehouse query, so they belong
on a *window* change; `usePredict` touches no database and re-runs per
keystroke as the analyst drags a lever. `useProjection` returns the **baseline**
curve only — the scenario's second curve is arithmetic over it and the
`usePredict` result, composed client-side so a lever edit costs no query.

```tsx
import { useBaseline, useProjection, usePredict } from "@oxy-hq/sdk";

function Scenario({ lever, delta }: { lever: string; delta: number }) {
  const period: [string, string] = ["2025-08-01", "2025-08-31"];
  const baseline = useBaseline({
    roots: [lever],
    time_dimension: "orders.order_date",
    period
  });
  // Fitted coefficients go in verbatim, refusals included — filtering them here
  // would just be a second place for client and server to disagree.
  const predicted = usePredict([{ measure: lever, delta }], {
    values: baseline.data?.values,
    coefficients: baseline.data?.fitted
  });
  const projection = useProjection({
    roots: [lever],
    time_dimension: "orders.order_date",
    // Its own, much longer window: the forecaster refuses under eight seasonal
    // cycles, so reusing the baseline's month would refuse every curve.
    period: ["2024-09-01", "2025-08-31"],
    granularity: "day",
    horizon: 30
  });

  return <p>{projection.data?.series.length} curves, {predicted.data?.impacts.length} impacts</p>;
}
```

A fuller worked example (graph + opportunity + RCA + streaming driver tree) is
in [examples/world-model-analysis.tsx](examples/world-model-analysis.tsx).

### Metric Tree (client-class)

For non-React / API-key callers, the programmatic `MetricTreeClient` and
`AnomaliesClient` (and all related types) are available from the package root
— see [metricTree.ts](src/metricTree.ts), [anomalies.ts](src/anomalies.ts), and
[examples/metric-tree.ts](examples/metric-tree.ts).

Hooks fail loudly if called outside `<OxyAppProvider>`. The default fetcher
sends `credentials: "include"` so same-origin (served-by-oxy) calls carry the
session cookie automatically.

### Workspace shell (`@oxy-hq/sdk/shell`)

The Oxygen workspace chrome — the 48px icon rail and universal top bar the
main web-app renders — as reusable components, so your app reads as part of
the same product. The main web-app consumes these exact components.

```tsx
import { OxyAppProvider } from "@oxy-hq/sdk";
import { OxyShell } from "@oxy-hq/sdk/shell";
import "@oxy-hq/sdk/shell.css";

export function App() {
  return (
    <OxyAppProvider>
      <OxyShell>
        <Dashboard />
      </OxyShell>
    </OxyAppProvider>
  );
}
```

`OxyShell` bootstraps from `GET /api/projects/:id/shell-context` (workspace
identity, sibling apps, host-aware navigation URLs) and degrades gracefully:
if the endpoint is unavailable (older server), your app renders unchromed.

| Export | What it does |
| --- | --- |
| `OxyShell` | Wired frame: rail + top bar + content column around your app. Slots: `topBarExtra`, `railBottom`, `hideTopBar`, `pageLabel`. |
| `useShellContext()` | The raw shell bootstrap payload (`{ data, loading, error }`). |
| `ShellRail`, `RailItem` | Presentational icon rail — props only, router-free. |
| `TopBar`, `Breadcrumb`, `SystemIndicator`, `WorkspaceClock` | Presentational top bar pieces. |
| `WorkspaceTile`, `OxyMark`, `OxygenFactoryMark` | Branding primitives. |
| `workspaceLogoUrl(apiBaseUrl, wsId, version?)` | Workspace logo endpoint URL builder. |

Styling: `shell.css` is namespaced (`oxy-shell-*`) — no Tailwind required, no
global styles leak into your app. It follows your design tokens when present
(`--sidebar-background`, `--foreground`, …) and falls back to the Oxygen
defaults. Dark mode: put a `.dark` class on any ancestor.

## Who is using the app

Two identity surfaces, and the difference between them is the difference between
a decision and a greeting.

**`ctx.user`, inside an Oxy Function — authoritative.** Assembled server-side per
invocation from the authenticated session, so nothing on it is client-supplied.
This is where a check that matters goes:

```ts
import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";

export default async function exportAll(req: OxyFunctionRequest, ctx: OxyFunctionContext) {
  if (ctx.user.appRole !== "admin") {
    return Response.json({ error: "forbidden" }, { status: 403 });
  }
  return Response.json({ rows: await dump(ctx) });
}
```

| Field | Notes |
| --- | --- |
| `id`, `email`, `orgId` | Always present. `orgId` is the tenant boundary for anything you query. |
| `name`, `picture` | Display identity. Absent on schedule/Airway runs. |
| `appRole` | `"admin"` \| `"member"` \| absent. **The one to gate on** — an app grant (direct or via a team), with org-officer / Oxy-staff break-glass. Fails closed. |
| `orgRole` | `"owner"` \| `"admin"` \| `"member"` \| absent. Informational: explain ("ask your org admin"), label, route. Not a gate — org standing and app standing are different things. |
| `teams` | Org teams they belong to, name-sorted, scoped to this org. Descriptive — a team only grants anything through an app team grant, which `appRole` already reflects. |
| `kind` | `"user"` \| `"system"`. |

`teams` and `kind` are typed optional because a server older than 2026-08-21
doesn't send them — use `ctx.user.teams?.some(...)`. For `kind` there is no safe
inference on such a server (`=== "system"` misses a cron, `!== "user"` misfires
on a person), so if you support one, mark the schedule's configured `input`
instead of guessing.

**Background runs have no caller to attribute them to.** A schedule tick, an
Airway step, and an operator's manual *Run now* all run under the org owner's
`id` with `kind: "system"`, every caller field absent, and a synthetic
`schedule+<fn>@system.oxy` email — but `appRole` still reads `"admin"`, since
they carry owner authority. Note the manual case: a person did click, and there
is still nobody to reach, because the triggering operator isn't carried through
the job queue. A function wired to both a route and a background trigger must
branch on `kind`, not on the email:

```ts
if (ctx.user.kind === "system") return runRollup(ctx);   // no one to email
await ctx.email.send({ to: ctx.user.email, subject: `Hi ${ctx.user.name}`, html });
```

**`useShellContext()`, in the bundle — display only.** `data.user` is
`{ name, email, picture } | null`, and it deliberately carries no role at all.
Use it for an avatar or a greeting. Hiding a tab with it is fine; the endpoint
behind that tab is what actually has to say no.

### Testing a function (`@oxy-hq/sdk/testing`)

A typed test context for an Oxy Function's unit tests, so a test meets the
host's real gates instead of a hand-rolled fake that is more permissive than
production:

```ts
import { createTestContext, type HostOp } from "@oxy-hq/sdk/testing";
import manifest from "../oxy-app.json";
import handler from "./notify";

const t = createTestContext(manifest, {
  function: "notify",
  databases: { warehouse: { dialect: "clickhouse", kind: "customer" } }
});

await t.run(() => handler({ method: "POST", headers: {}, body: "{}" }, t.ctx));

t.ops();                              // HostOp[] — first-call order, de-duplicated
t.calls;                              // every host op, in order: { op, args, outcome, message?, result? }
t.callsTo("warehouse.insert");        // one op's calls
t.state.warehouse("warehouse").rows("events");
t.override("warehouse.upsert", async () => ({}));   // replace one host op, typed by name
```

What it enforces, in the host's own words (every refusal is quoted from the
host's source and held there by a Rust drift test): the manifest's capability
gates (`secrets.write`, `email.send`, `org.read`, `storage.read` /
`storage.write`, `oltp`, `airhouse`); the `destinations` allowlist and the
customer-warehouse rule for `ctx.warehouse` writes and `ctx.tx`; `ctx.tx` on a
non-Postgres database; `warehouse.upsert` where `ON CONFLICT` does not parse
(on Airhouse it refuses in its own words, because there the *engine* refuses);
`ctx.fetch`'s SSRF allowlist and byte cap. A refusal is an `Error` named
`HostError` whose message is `"<surface>: <host message>"`, as the runtime
throws it.

Rows come from a byte copy of the platform's shape zoo, so a ClickHouse
`UInt64` above `i64::MAX` arrives as a string in a test as it does in
production: `t.state.warehouse(db).zoo()` creates the zoo table (column `cNNN`
is case NNN), `.table(name, { id: "UInt64" }).insert(rows)` renders SQL-side
values through the case for each type, and `.raw(name, rows)` takes JS values
as-is — every read of those is marked `source: "author"` on its `HostCall`.
`ctx.oltp` and `ctx.airhouse` have the same stores; `ctx.fetch` answers from
`t.state.fetch.on(url, { status, body })`; `ctx.org.*` from
`t.state.org.people` / `.places` / `.assignments`.

`t.run(fn)` evaluates `fn` with the isolate's absent globals removed —
`Buffer`, `TextEncoder`, `TextDecoder`, `Blob`, `File`, `FormData`, `crypto`,
`process` — and `btoa` / `atob` replaced by the runtime's own, so a handler
that reaches for one throws the `ReferenceError` it throws in production. It
is per call; a vitest environment that removes them for a whole file may
follow later.

What it is not: a server, a real engine, or a replacement for `oxyc checks
run`. The context's SQL is a small subset (`CREATE TABLE`, `INSERT … VALUES`,
`SELECT … FROM … WHERE … = …`, `DELETE`, `UPDATE`), and it says so — an
unrecognised statement throws a `TestContextError` naming `t.override` and
`t.state.<store>.raw` as the ways out, rather than answering something the host
would not. `UPDATE … SET` follows the same rule as a projection: an assignment
the subset cannot read as a value (`SET n = n + 1`, `SET at = now()`) is
refused rather than stored as its own SQL text. Nothing here reaches the
network or the filesystem.

It also needs Node: `@oxy-hq/sdk/testing` imports `node:crypto` at module load
(the context's `ctx.crypto` is the real one), so it runs under vitest's `node`
environment and not under browser mode. The rest of the SDK does not.

## Docs

- Hands-on dev + deploy guide: `docs/local-development.md` in the
  [`oxy-hq/customer-apps`](https://github.com/oxy-hq/customer-apps) repo.
- SDK flow reference: `docs/sdk-flow.md` in that repo.
- Platform internals: `internal-docs/customer-apps.md` and
  `internal-docs/custom-apps-user-identity.md` in oxygen-internal.


## License

MIT
