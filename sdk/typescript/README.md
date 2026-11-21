# @oxy-hq/sdk

React SDK for building **custom-app bundles** on the [Oxy](https://oxygen-hq.com)
platform. A bundle is a normal Vite + React app that reads from its linked oxy
project — raw SQL, the semantic model, agents, and procedures — through a
small set of hooks, plus a couple of drop-in components.

> **v2 is a complete rewrite.** The v1 stack (`OxyClient` / `OxySDK`, the
> Parquet/DuckDB-WASM reader, postMessage auth) is gone. Bundles now talk to
> `/api/projects/:id/*` exclusively. See `CHANGELOG.md`.
>
> Data Apps (`.app.yml`) are gone from Oxygen, and so is the SDK code that
> called their `/apps/*` routes. A bundle replaces a Data App with a custom
> app that reads through `useQuery` / `useSemanticQuery`.

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
{
  "$schema": "https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json",
  "schemaVersion": 2,
  "slug": "store-pulse",
  "orgSlug": "acme",
  "name": "Store Pulse"
}
```

The contract is one type, `OxyAppManifest`, and everything else derives from
it: `schema/oxy-app.schema.json` (generated — `pnpm gen:manifest-schema`,
pinned by a test; `$schema` gives your editor completion, and
`./node_modules/@oxy-hq/sdk/schema/oxy-app.schema.json` is the installed
copy), and `@oxy-hq/sdk/manifest` — a pure-TS entry with the types, the
schema, the two server name rules (`isValidSlug`, `isValidFunctionName`) and
`validateOxyAppManifest(raw)`, which `oxyc validate`, `oxyc publish`,
`@oxy-hq/vite-plugin` and this SDK's own loader all run. An unknown field is
a warning from it, never a refusal.

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
| `useAskAgent()` / `askAgent(fetcher, projectId, agentId, question)` | One call: start an ask and wait (up to 45 s, `waitSeconds`) for its `answer`, `error` or clarifying `question`. `status: "running"` when the wait runs out first. |
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
| `OxyShell` | Wired frame: rail + top bar + content column around your app. Slots: `topBarExtra`, `railBottom`, `hideTopBar`, `plainClock`, `pageLabel`; `modePin` (see the theme's light and dark, below). |
| `useOxyModePin(mode)` | Pin one screen to `"light"` or `"dark"` while it is mounted, and hide the mode switch (theme.css only). |
| `useOxyTouchTargets(false)` | Turn the shell's 44px touch tap areas off while mounted, for a store's enrolled tablet (`<OxyShell touchTargets={false}>` from inside the shell). |
| `useShellContext()` | The raw shell bootstrap payload (`{ data, loading, error }`). |
| `ShellRail`, `RailItem` | Presentational icon rail — props only, router-free. |
| `TopBar`, `Breadcrumb`, `SystemIndicator`, `WorkspaceClock` | Presentational top bar pieces. |
| `WorkspaceTile`, `OxyMark`, `OxygenFactoryMark` | Branding primitives. |
| `workspaceLogoUrl(apiBaseUrl, wsId, version?)` | Workspace logo endpoint URL builder. |

The top-bar clock in `OxyShell` is a button that opens **Today**: the viewer's
widget cards from every app they can open, needs-attention first, each row a
plain link, with a footer link to Oxygen home. It is fetched the first time the
panel opens (`GET /api/home/widgets?org=<slug>`) and a failure only shows
"Couldn't load Today." in the panel. Nothing to configure; `shell.css` styles it.
In the phone layout (`PHONE_LAYOUT_QUERY`) each top-bar control, the menu button,
the clock, Ask and a button or link your `topBarExtra` puts directly in the bar,
answers a tap within 44px of its centre; nothing drawn grows. A tablet and a desktop
keep the bar exactly as it is. The reach is that control's `::before`, so in the phone
layout the shell owns the `::before` of a button or link placed directly in the bar:
put a badge on such a control in a child element, not in its `::before`.
In the phone layout, Ask Oxygen opens as a full-screen layer over the app (the
app keeps its width and is inert behind it) rather than docked beside it, and
Back closes it; a tablet and a desktop keep the docked side panel.
In the phone layout an open Today panel also holds a history entry, as a
`PhoneSheet` does, so the phone's Back closes it and the app stays; a row or the
home link replaces that entry rather than leaving it behind. A tablet and a desktop
hold none.
A screen that is not one person's, such as a store's shared tablet, passes
`<OxyShell plainClock>` to keep the clock a plain time: no Today button or panel,
and no widgets read.

Styling: `shell.css` is namespaced (`oxy-shell-*`) — no Tailwind required, no
global styles leak into your app. It follows your design tokens when present
(`--sidebar-background`, `--foreground`, …) and falls back to the Oxygen
defaults. Dark mode: put a `.dark` class on any ancestor.

### Theme (`@oxy-hq/sdk/theme.css`)

The Oxygen design standard for every custom app is the
[reference app](https://github.com/oxy-hq/reference-app) (`oxy-hq/reference-app`, `main`).
`theme.css` is that app's design as CSS custom properties, and importing it applies
it: the reference's surfaces, three ink tiers below the primary, the frame line
(`--border-subtle`) apart from the 3:1 control line (`--border`), one brand blue, the
status family (ink, tint, line, chart cut), square corners (every `--radius-*` is 0;
only a status dot is round), the system face first (`-apple-system, BlinkMacSystemFont,
"Inter", system-ui, …`, nothing downloaded), 32px desk and 44px touch controls
(`--control-h-sm` / `--control-h-lg`), the 1.5px `--muted-foreground` focus ring, and
one elevation for what floats. Light is the default; `.dark` (or
`data-oxy-mode="dark"`) on `<html>` switches to dark.

```ts
// main.tsx, above your own CSS
import "@oxy-hq/sdk/theme.css";
import "./index.css";
import "@oxy-hq/sdk/shell.css";
```

- **Your own tokens still win.** Every selector in `theme.css` sits in `:where()`, so a
  `--border` or `--radius` your app declares overrides the theme in either import
  order (Tailwind's `@layer theme` does not: an imported theme beats it). Adopting the
  theme means importing it and **deleting** your own palette, radii and fonts, and
  any `--oxy-shell-*` overrides and `!important` corner rules aimed at the shell or
  kit.
- **The shell and the phone kit read it.** Corners come from `--radius-control`,
  `--radius-surface`, `--radius-overlay`, `--button-radius`, `--card-radius`,
  `--input-radius` and `--badge-radius` (each clamped at 0); the rail, top bar, tab bar,
  sheets, cards and KPI tiles draw their edges on `--rail-border`, `--topbar-border` and
  `--card-border` (the frame line); the Ask button and the kit's chips, segments and
  secondary buttons on `--button-secondary-border` (the reference's secondary-button
  edge), while fields keep `--border`; the rail and bars sit on `--rail-bg` and
  `--topbar-bg`; the top bar's Ask is the reference's secondary button, like the app
  buttons beside it (`btnCompactSecondary`: filled `--btn-surface`, hover
  `--btn-surface-hover`, full ink, its ⌘K hint on a `--muted` chip in the muted ink, at
  its old size); the nav drawer's scrim is the theme's overlay (below); touch targets are
  `--control-h-lg`; and the shell's type follows the kit's, `--font-sans`. Every focus
  ring is the theme's one ring (`--focus-ring-width`, `--focus-ring-color`: 1.5px, at full
  strength), and every focused control also draws a transparent outline, which is what
  forced colours show. The kit's primary button is the brand fill (`--button-primary-bg`,
  `--button-primary-fg`), its secondary button `--secondary` (the reference's
  `--btn-surface`) and its selected segment `--segment-selected` on a `--segment-track`
  track; the phone tab bar's current tab, icon and label, is the accent blue, the theme's
  `--link` (7.77:1 on the bar light, 7.09:1 dark), the one selection drawn in blue, while
  the desk's navigation, tabs and segments keep a monochrome selection; `OxyChat`'s Ask is
  the primary pair too, its field and buttons on
  `--input-radius` and `--button-radius`. In the top bar the breadcrumb's "/" is the muted
  ink at full strength, its home link takes the one ring, and the Today clock is drawn in
  the shell's sans face with tabular figures. The kit's tones (`good`, `warn`, `bad`,
  `neutral`) are the status family the desk draws, so one status reads one way on both
  layouts: a `PhoneCard` status pill is the reference's chip pair (`--status-*-bg` with
  `--status-*`, and for neutral `--status-neutral-bg` with `--dim-foreground`), a
  `PhoneKpiStrip` delta is the status ink (`warn`: `--status-warning`; neutral:
  `--muted-foreground`), and a `PhoneRow`
  dot is `--dot-*` (neutral: `--status-neutral-series`), in place of the kit's own mix of
  the destructive and success tokens. `--oxy-phone-tone` itself is unchanged.
- **Names for the reference's own parts.** `--segment-track`, `--segment-selected`,
  `--btn-surface`, `--btn-surface-hover` and the status dots `--dot-success`,
  `--dot-info`, `--dot-warning`, `--dot-error` carry the reference's values (light
  `--dot-success`, `--dot-warning` and `--dot-error` are corrected, with a comment, to clear
  3:1 on every light ground a dot sits on, a selected row and the selected segment
  included), so an app
  that draws a segmented strip, a filled secondary button or a status dot deletes its
  own copies. So do these, at the reference's values in both modes: `--status-info`,
  `--status-info-bg` and `--status-info-line` (info's ink, tint and edge; the reference
  retired info as a status, so they keep an app's existing calls in colour), `--shadow-elevated`
  (its name for the one shadow, the same as `--shadow-overlay`), `--select-chevron` (the
  12x8 chevron image for a native `<select>` with `appearance: none`) and the dots' glows
  `--dot-success-glow`, `--dot-info-glow`, `--dot-warning-glow`, `--dot-error-glow`.
- **Overlays blur the page behind them.** A design-system rule beyond the reference, whose
  dialogs only dim: every scrim is `--scrim` (light black at 0.35, the reference's; dark
  black at 0.5) and blurs the page by `--scrim-blur` (8px). Under
  `prefers-reduced-transparency: reduce` the blur is 0px and the scrim stays (Chrome and
  Edge report that setting; Safari and Firefox do not yet, so they keep the blur). Which to
  use:
  - **A native modal `<dialog>`** (the reference's `Modal`, opened with `showModal()`):
    nothing. `theme.css` blurs every dialog's `::backdrop` and never sets its colour, so
    your own `::backdrop` rule or `backdrop:bg-*` stays; make that colour the theme's
    (`dialog::backdrop { background: var(--scrim); }` or `backdrop:bg-(--scrim)`).
  - **Any other scrim** (a fixed `div`, a Radix or shadcn `DialogOverlay` /
    `SheetOverlay`): put the class `oxy-scrim` on it, which gives it `--scrim` and the
    blur, and drop its own `bg-*` (a utility would win over the class's colour). In
    Tailwind without the class: `bg-(--scrim) backdrop-blur-(--scrim-blur)`.
  - Both rules sit in `@layer theme`, so any rule you write wins over them; set
    `--scrim-blur` yourself only to change the blur everywhere (you then own the
    reduced-transparency case too).
  - The shell's own: the nav drawer (the phone menu) dims and blurs this way. `PhoneSheet`
    and the Ask dock on a phone cover the whole screen, opaque, so nothing shows behind
    them; on a desk the dock sits beside the app, not over it.
- **`--chart-1` … `--chart-5` are the reference's sequential blue ramp** (the same as
  `--chart-seq-1…5`: light `#5d73ff` to `#05081a`, dark `#3550ff` to `#ebedff`), not a
  categorical palette: neighbours of one ramp cannot be told apart as categories, so series
  that are categories read `--chart-cat-1…8`. An app that declares its own `--chart-*`
  keeps them.
- **Where the theme is not the reference, and why.** Each of these misses a contrast
  obligation at the reference's value, so the theme moves it the smallest step that clears
  it; the token's source (`src/theme/tokens/reference/primitives.tokens.json`,
  `palette.derived`, each marked "FIX, not the reference") says how it was measured:
  - dark `--brand` `#445dff` (the reference's `#3550ff`, 2.65:1 on the dark `--muted`, under
    the 3:1 a mark needs; `#445dff` reads 3.02, white on it 4.93), with `--brand-hover` and
    `--button-primary-bg` on it;
  - dark `--disabled-foreground` `#626268` (the reference's `#606066`, 2.45:1 on the dark
    field, under the theme's 2.5 floor for inactive text; `#626268` reads 2.53);
  - light `--status-warning-series` `#be8204` (the reference's `#ca8a04`, 2.72:1 on the
    canvas, under 3:1 for a chart mark);
  - light `--dot-success`, `--dot-warning` and `--dot-error` (`#059341`, `#a5770d`,
    `#eb3c3e`, 3:1 on every light ground a dot sits on), and their glows, which are those
    dots again at 0.7 by the reference's own rule.

  Two more are a design choice, not a contrast fix: the overlay blur. The design system keeps
  the blurred overlays the Poke House apps had before it (decided 17 Nov), so `--scrim-blur` is
  8px where the reference has none, and dark `--scrim` is black at 0.5 (the reference's
  0.35 leaves a dialog's card at 1.16:1 over the dimmed page and 0.5 at 1.17, both over the
  contract's 1.15 floor for a scrim that also blurs; 0.5 sets the dialog off a step more and
  the blurred page still reads through it, where 0.92, the value before the blur, hid it).
- **Without it nothing moves.** An app that does not import `theme.css` renders as it
  did on 2.28: every name above is read as `var(--theme-name, <the old value>)`, and the
  old value still follows your `--radius`, `--border`, `--font-inter` and
  `--oxy-shell-*`. The parts that read names apps also set for themselves (`--secondary`,
  `--segment-track`, `--segment-selected`, the status pairs, dots and `--dim-foreground` for
  the kit's tones, `--link` for the tab bar's current tab, `--muted-foreground` for
  the breadcrumb's "/", and `--btn-surface`, `--btn-surface-hover` and `--foreground` for
  the Ask button), and the keyboard hint's 11px floor, are
  switched on the theme's own marker (`--oxy-theme-contract`) instead, so declaring them
  without the theme changes nothing either. The overlay blur reads `--scrim-blur` with no
  fallback, so without the theme it is `none`. The clock's face and the home link's ring
  switch on the same marker through a style query; a browser without style queries keeps
  2.28's monospace clock and its own focus outline there.
- The token list, with each value's source line in the reference app, is
  `src/theme/tokens/reference/` (contract: `src/theme/contract/contract.json`). The
  optional JS entry `@oxy-hq/sdk/theme` switches the mode (`applyOxyTheme({ mode })`).

#### Light and dark: one setting, Oxygen's

With `theme.css` imported, your app shows the signed-in person's light/dark mode, the
same one Oxygen home shows, and changing it in either place changes it in both:

- **`OxyShell` applies it.** The shell context carries the person's `mode` (`"light"`,
  `"dark"`, `"system"`, or `null` until they choose, which shows Oxygen's default, light).
  The shell applies it (`data-oxy-mode` and `.dark` on `<html>`), re-reads it whenever the
  tab is shown again (`GET /api/projects/:id/shell-context/mode`), so a change made in
  Oxygen or another app shows up, and follows the device under `system`.
- **One switch, in the same place in every app.** A Light / Dark / System switch sits at
  the bottom of the rail, where Oxygen keeps its user menu, and at the bottom of the nav
  sheet on a phone. A choice applies at once and is written to the person
  (`PUT /api/projects/:id/shell-context/mode`); if the server refuses it, the old mode
  comes back.
- **Staging and sandbox previews never change the person's real mode.** On a staging or
  sandbox host (`window.__OXY_APP__.environment` is `staging` or `dev-<handle>`) the switch
  still works, for that preview only: the choice is kept in that origin's copy and never
  sent (the server refuses the write there as well), and it stays put when the tab is shown
  again or the page reloads, so a tester can check dark mode on staging. Until a tester
  chooses, a preview shows the person's mode as read.
- **The browser's chrome follows.** Whenever the shell or the pre-paint applies a mode, pins
  included, every `<meta name="theme-color">` (a phone browser's toolbar, an installed app's
  status bar) takes the page ground of that mode, `--background` as `<html>` computes it
  (light `#f6f6f7`, dark `#0a0a0a`); a page with none gets one. It follows any later change
  of `.dark` or `data-oxy-mode` on `<html>` too, whoever makes it, while the shell is mounted
  and, with the pre-paint, for the page's life. Keep the meta in `index.html` for the first
  paint, and delete a hook of your own that sets it. `syncOxyThemeColor()`
  (`@oxy-hq/sdk/theme`) sets it once, on demand. Without `theme.css` the meta is left alone.
- **No flash.** This origin keeps a copy (`localStorage["oxy-person-mode"]`), and
  `OXY_THEME_PREPAINT` (`@oxy-hq/sdk/theme`) applies it before the first paint. Put it in
  an inline script in `<head>`, anywhere there, before or after your stylesheets. With
  Vite:

  ```ts
  // vite.config.ts
  import { OXY_THEME_PREPAINT } from "@oxy-hq/sdk/theme";
  // in `plugins`:
  {
    name: "oxy-theme-prepaint",
    transformIndexHtml: () => [{ tag: "script", children: OXY_THEME_PREPAINT, injectTo: "head" }]
  }
  ```

  Why `<head>`: a browser paints nothing until the stylesheets in `<head>` have loaded and
  `<body>` has begun. A script at the top of `<body>` runs after both, and Chrome can paint
  the empty page in between: one light frame for a dark person. In `<head>` the mode is on
  `<html>` before anything paints. Ahead of your stylesheets the script cannot yet tell
  whether `theme.css` applies, so it puts the mode on at once and, just before the first
  frame, settles what it could not know yet: a pin set after it, the platform's
  `window.__OXY_APP__` (added at the end of `<head>`), and `theme-color`. If `theme.css`
  never applied, it takes the mode back off. Without the script the shell applies the mode
  when it first renders.
- **Pin a mode where it must not move.** `<html data-oxy-mode-pin="light">` in your
  `index.html` pins the whole app, and so does a script in `<head>` that sets it before the
  first frame (the pre-paint reads the pin again there). For one screen, such as a store's
  shared tablet, pass `<OxyShell modePin="light">`, or call `useOxyModePin("light")` in a
  screen that renders without the shell. A pin wins over the person's mode and hides the
  switch; when the screen unmounts, the person's mode returns. While a pin is in force the
  shell leaves this origin's copy as it is, so a person who signs in on a pinned screen (a
  store's shared tablet) leaves no mode behind for the next load to paint; their own,
  unpinned screen keeps it.
- **When it does nothing.** Without `theme.css` the shell applies no mode and draws no
  switch, so an app that never imported the theme, its own `.dark` styles included, looks
  as on 2.28.1. A server older than the field sends no `mode`: the app keeps its own mode
  and shows no switch. A crew (PIN) sign-in has no Oxygen account, so it gets neither. In
  a staging preview, `?oxy-mode=` still wins for that session.
- An app that sets `.dark` on `<html>` itself and imports the theme now follows the person
  instead; pin `dark` to keep it dark-only.

#### Touch screens: 44px on the shell's own chrome

With `theme.css` imported, on a touch screen (`pointer: coarse`) the shell's rail entries (the
mode switch among them), the Today clock, the Ask button and the update pill each answer a
tap within an invisible area at least 44px each way, centred on them, in the phone layout's
top bar as on a tablet; nothing drawn grows, and
the mode switch's menu rows are a real 44px. One thing moves: the rail's entries sit 44px apart
(12px gaps instead of 4px), so each owns its 44px and no entry's area takes the edge of the next
one. A screen that is not a
person's desk, such as a store's enrolled tablet, turns the areas off with
`<OxyShell touchTargets={false}>`, `useOxyTouchTargets(false)` in a frame around the shell, or
`<html data-oxy-touch-targets="off">` for a whole app; there the breadcrumb's home link, which
fills the top bar's height on any touch screen since 2.27, keeps its drawn size too. Without the
theme, on a fine pointer, and in a browser without CSS style queries, the chrome is as it was.

Motion: `shell.css` loads `@oxy-hq/sdk/motion.css`, the motion language
Oxygen itself uses, so your app moves like Oxygen without doing anything:

- **Tokens** on `:root`: `--oxy-duration-instant/fast/base/slow/focal`
  (80 / 120 / 160 / 240 / 600 ms) and `--oxy-ease-out/in/in-out`. Arrivals use
  `ease-out`; departures use `ease-in` and run faster.
- **shadcn/Radix overlays** (dialogs, sheets, menus, popovers, selects,
  tooltips) are timed from their `data-slot` and `data-state`, and settle out
  of their trigger (scale 0.97, a 4px nudge). Don't add `duration-*` to them.
  A shadcn button gives slightly under a press. That rule sits in
  `@layer components`, so your own transition and scale classes on a button
  win; include `scale` in the button's transition list to ease the press.
- **Reduced motion** keeps fades and drops all travel.

In a Tailwind app, map the tokens so your own utilities match (the
`create-oxy-app` templates do this in `src/index.css`):

```css
@theme {
  --ease-out: var(--oxy-ease-out);
  --ease-in: var(--oxy-ease-in);
  --ease-in-out: var(--oxy-ease-in-out);
  --default-transition-duration: var(--oxy-duration-base);
  --default-transition-timing-function: var(--oxy-ease-out);
}
```

Without the shell, `import "@oxy-hq/sdk/motion.css"` on its own.

### Phone kit (`@oxy-hq/sdk/shell`)

A desktop layout stacked into one column passes a breakage check and still
fails a person holding a phone: a table you scroll sideways, a row of tabs cut
mid-word, the answer under a map, explanations that only open on hover. The
phone kit is the set of pieces an app's **phone view** is made of, styled by
`shell.css` with the shell's tokens (light, dark via a `.dark` ancestor, and the
host's own tokens when it has them). Every app follows one pattern: keep the
desktop component as it is and render a phone view beside it, from the same data
hooks and actions.

```tsx
import {
  PhoneCard, PhoneCardList, PhoneKpiStrip, PhonePage, PhoneSheet, PhoneTabBar,
  usePhoneLayout
} from "@oxy-hq/sdk/shell";

function Orders() {
  const data = useOrders(); // the app's existing hooks
  const phone = usePhoneLayout();
  return phone ? <OrdersPhone {...data} /> : <OrdersDesktop {...data} />;
}

function OrdersPhone({ stores, totals }: OrdersData) {
  const [tab, setTab] = useState("today");
  const [open, setOpen] = useState<Store | null>(null);
  return (
    <PhonePage
      scrollKey={tab}
      tabBar={<PhoneTabBar items={TABS} activeId={tab} onSelect={setTab} />}
    >
      <PhoneKpiStrip items={totals} />
      <PhoneCardList summary={`${stores.length} stores`}>
        {stores.map((s) => (
          <PhoneCard
            key={s.id}
            title={s.name}
            status={{ label: s.posted ? "Posted" : "Late", tone: s.posted ? "good" : "warn" }}
            figures={[{ label: "Sales", value: s.sales }, { label: "Net", value: s.net }]}
            onClick={() => setOpen(s)}
          />
        ))}
      </PhoneCardList>
      <PhoneSheet open={open !== null} onClose={() => setOpen(null)} title={open?.name}>
        …
      </PhoneSheet>
    </PhonePage>
  );
}
```

| Export | What it is |
| --- | --- |
| `usePhoneLayout()` | `true` on a phone upright (narrower than 640px) or on its side (a touch screen no taller than 500px); re-renders when that changes, `false` on the server. `PHONE_LAYOUT_QUERY` is the media query, for code outside React. |
| `PhonePage` | The frame: 16px gutters (plus safe-area insets), 12px rhythm. It scrolls with the shell's main area or the document, never in a box of its own. `tabBar` and `stickyAction` dock at the bottom of the screen and the content clears them, also under wrappers that hug their content (padding on a wrapper insets the dock, so for an edge-to-edge dock render the page without one); a new `scrollKey` scrolls back to the top. |
| `PhoneTabBar` | Bottom navigation, 56px plus the home indicator. Up to five slots; past five, four and a **More** slot that lists the rest in a sheet. `items: { id, label, icon?, badge?, badgeTone? }[]`, `activeId`, `onSelect`. A badge is the shell's red unless you give it a `badgeTone` (`good`, `warn`, `bad`, `neutral`): then it is drawn in that tone, and with the theme as the reference's tab count, the tone's status pair or, for `neutral` (a count that is just a count), `--muted` in the muted ink. An attention count takes the worst status it holds. More's dot takes the strongest tone behind it (error, then warning, then neutral; success counts as neutral) once every badge there has one. `historyEntry` (off by default) makes the phone's Back work as in a native app's bottom navigation: from any section but the first it calls `onSelect` with the first item's id, and from the first it leaves; the bar holds one history entry while another section shows (one, however many tabs are tapped), only in the phone layout. Turn it on when the section lives in state or in the URL by replace. Leave it off when `onSelect` pushes history (a route or `?tab=` per section): Back would swap between two sections forever. |
| `PhoneSheet` | A full-screen sheet with a 44px Back, a scrolling body and an optional `footer`. Back and Escape call `onClose`; focus goes in on open (with the theme, Back shows its ring only for a sheet opened from the keyboard) and back to the control that opened it on close; the page behind does not scroll. The phone's Back also calls `onClose` and leaves the page where it was: an open sheet holds a history entry on the same URL. A sheet that opened by pushing its own entry (a route, `?item=`) passes `historyEntry={false}`. |
| `PhoneStickyAction`, `PhoneButton` | The action a screen exists for, pinned above the tab bar. Buttons are 48px; `tone` is `primary`, `secondary` or `danger`. |
| `PhoneKpiStrip` | Headline numbers, two per row: label, value, a `delta` coloured by `tone`. A tile with `onClick` is a button. |
| `PhoneSection` | An 11px label with an optional `action`, then its content. Rows directly inside join into one list. With the theme the label starts on the page title's and the cards' left edge. |
| `PhoneRow` | A 56px list row: status dot (`tone`), title, subtitle, a `trailing` slot, and a chevron when it has `onClick`. With `onClick` the row's button is the row's full height, so a tap anywhere on it opens it; a control in `trailing` keeps its own tap. With the theme, a row anywhere in a `PhoneSheet` (under a `display: contents` wrapper too) keeps its whole height rather than squeezing to 56px. |
| `PhoneCard`, `PhoneCardList` | A table row as a card: title with a status pill, subtitle, up to three figures, a `trailing` action (it keeps its own tap). A figure's value is never cut through a glyph: its parts wrap, text alone that still does not fit ends in an ellipsis, and an element of text wraps inside itself. With the theme, one figure column per figure. The list puts a one-line `summary` on top and goes to two columns on a phone on its side. |
| `PhoneSegmented` | Two to four 44px segments (List / Map) as a radio group: equal while every label fits, a long label keeps its width, and a row too wide for the screen scrolls, faded at the edge with more, the selected option kept in view. |
| `PhoneFilterChip` | A 44px chip: the one filter control in a page header ("Oct 2026 · All companies"), with the count of filters set; it opens your filters `PhoneSheet`. |
| `PhoneHint` | An ⓘ (24px to see, 44px to hit) that explains a number on a tap, and on hover or keyboard focus where there is a mouse. It replaces a hover-only tooltip. |

Inside a `PhonePage` or `PhoneSheet` the kit keeps a floor: body text 13px or
more, labels 11px or more, actions 44px or more, and small fields raised to 16px
so iOS does not zoom into them (a field set larger keeps its size; `text-[11px]`
and your own small `font-size` rules are not raised, so don't use them on a field).
Corners follow your `--radius` (at `0`, every control is square; status dots stay
round) and text your `--font-sans`. Never put a sideways-scrolling table on a phone: rows become
`PhoneCard`s, details open in a `PhoneSheet`, sections go in the `PhoneTabBar`,
and the first screen answers what the person opened the app for.

### Desk UI (`@oxy-hq/sdk/ui`, `ui.css`, `tailwind.css`)

The desk half of the design system: the reference app's gallery (components, the
recipes, tones, icons) as React pieces and plain CSS, so a desk screen needs no
copied `ui.ts` and no Tailwind. Every class is `.oxy-ui-*`, every rule reads
`theme.css` tokens, both modes.

```ts
// src/main.tsx, once, in this order
import "@oxy-hq/sdk/theme.css";
import "@oxy-hq/sdk/shell.css";
import "@oxy-hq/sdk/ui.css";
```

```tsx
import { Button, PlusIcon, ui } from "@oxy-hq/sdk/ui";

<section className={`${ui.card} ${ui.panel}`}>
  <header className={ui.panelHeader}>
    <span className={ui.eyebrow}>Orders</span>
    <Button variant='text' icon={<PlusIcon />}>Add a line</Button>
  </header>
  <input className={ui.field} placeholder='PO number' />
  <Button variant='primary' busy={saving} busyLabel='Saving…'>Save</Button>
</section>
```

| Export | What it is |
| --- | --- |
| `ui` | The recipes as class names, under the reference's own names (`ui.card`, `ui.btnCompact`, `ui.spacingSm`, `ui.sheetRow`, …) so moving an app off its `ui.ts` is `card` -> `ui.card`, and by family (`ui.btn.primary`, `ui.space.md`, `ui.control.sm`, `ui.table.cell`, `ui.text.xs`, `ui.chip.warning`). `ui.rowState(selected)` gives a row one state. |
| `Button` | Three variants (`primary`, `secondary`, `text`) at two sizes (`sm` 32px, `lg` 44px); `icon`, `iconOnly` (give it an `aria-label`), `inert` (the box as a `<span>`, for an action nothing is behind yet), and `busy` + `busyLabel`: the button keeps focus, says `aria-busy`, shows progress and ignores presses, a form submit included. `buttonClass(variant, size)` is the class matrix. |
| `chipTone`, `calloutTone`, `toneText`, `toneFill`, `strongestTone` | The status tones (`success`, `warning`, `error`, `info`, `neutral`) and the rule for which one a group's count wears (error, then warning, else neutral). `phoneToneOf` / `statusToneOf` translate to the phone kit's `good` / `warn` / `bad` / `neutral`. |
| `CaretRightIcon`, `CheckIcon`, `PlusIcon`, … `Glyph` | Phosphor regular, inlined; 16px by default, `className={ui.iconInline}` for 14px. |
| `cx` | Class joining (the shell's own). |

`ui.css` puts every rule inside `@layer oxy-ui`, ordered
`theme, base, oxy-ui, components, utilities`: above a Tailwind preflight, under
its utilities, so a utility class adjusts a piece and any unlayered app rule wins.
On a touch screen every desk control reaches 44px and every field takes 16px text
(decision 16); a screen that is not a person's desk turns that off as it does the
shell's, with `<html data-oxy-touch-targets="off">` or `useOxyTouchTargets(false)`.

A Tailwind 4 app adds the reference's utility names on top, so `bg-card`,
`text-dim-foreground`, `text-md` and `rounded-md` keep working on the theme. Import
it before Tailwind, so its first line fixes the layer order:

```css
/* src/index.css */
@import "@oxy-hq/sdk/tailwind.css"; /* @layer theme, base, oxy-ui, components, utilities */
@import "tailwindcss";
```

It maps 56 `--color-*` (the reference's 51, plus `sidebar`, `sidebar-border`,
`card-foreground`, `status-neutral-series` and `status-info-series`), 26 `--text-*`
(13 steps and their line heights) and 8 radius steps onto theme tokens, all `var()`,
inside `@theme inline`. Hand-written CSS still reads `var(--card)`, never
`var(--color-card)`.

The `--text-*` names are the reference's one type scale (decision 82), not
Tailwind's defaults: `text-sm` is 13px, `text-lg` 20px, and the steps above `3xl`
are display sizes (`text-4xl` is 64px, where Tailwind's is 36px). Where a figure would
grow, pick the step closest to its old size; never restate a size name in the app.

**A plain-CSS app layers its own element rules.** Every ui.css rule is in
`@layer oxy-ui`, and an unlayered rule outranks any layered one whatever its
specificity: a reset's `button, input { font: inherit; color: inherit }`, a
`:where(button):focus-visible` ring or a bare `h2 { text-transform: uppercase }`
would win over the pieces (their colours, rings and titles). Put them in
`@layer base` and declare the order first:
`@layer theme, base, oxy-ui, components, utilities;`. The same layering means a
utility class (or any unlayered app rule) always beats a recipe class on the same
element, whatever order the class list gives.

**New in 2.31.0** (each opt-in, except the changes listed after the table: a piece
that passes none of it is drawn as before):

| Piece | What it takes now |
| --- | --- |
| `Panel` | `header` (a whole header node), `title` optional, `headingLevel` (an `<h2>`...), `as="section"` / `"aside"` + `label` (a landmark, named by its heading without one), `foldable` with `defaultOpen` / `open` / `onOpenChange` (a caret text button on the title), `right` as a function of the fold, `density="compact"` (12px in, a 12px body, the eyebrow on the title alone), `bodyClassName` |
| `Sparkline` | `width` / `height`, `fluid` (the container's width, drawn 1:1), one series (`peerPct` optional; `sparkPoints(values)`), `area={false}`, `color`, `x` (points placed by a key such as a timestamp), `readout={{ format, periodLabel }}` (a Tab stop: arrows, Home / End and the pointer show a reading's value and period in a tooltip) |
| `StatusDot` | `wordHidden`: the dot alone where the words beside it say the state; the word stays for assistive tech and as the hover title |
| `DefaultTabs` | `disabled` on an option (passed over by the arrows, no click, focus kept), `stretch` (equal parts of the row), `hint` per tab (opened by keyboard focus too), a node `label` with `text` for the select; counts print grouped (`12,345`) |
| `ParentTabs`, `TopBarTabs` | `topBarFrom` `sm` (640), `lg` (1024) and `2xl` (1440) beside `md` and `xl` |
| `HoverHint` | `focusable`: the anchor is a Tab stop and carries the words. Renders without a DOM (server render, a node test) |
| `Stat`, `StatGrid` | `tipMark` (a focusable ⓘ on the figure's line on a desk; on by default for a tile with a `tip`, `tipMark={false}` opts out), `valueAt="foot"` / `"top"`, and the div's own props (`title`, `aria-label`, ...) |
| `MetricCell` | the `quiet` step (and a `quiet` threshold): the warning tint and ink at weight 400, a quieter warning under `warning` (600). Two warning tiers: notable -> `quiet`, elevated -> `warning`, severe -> `error` |
| `SidePanel` | `id` on the dialog |
| `useModalFocus` | `{ auto: true }` for a dialog drawn by hand: remembers the opener (even when the dialog autofocuses a field), moves focus in, traps Tab, gives focus back on close or unmount |
| `TryAgainButton` | Try again on its own, with the focus hand-on; Try again carries `data-oxy-try-again` |
| `LoadingBar` | `progress` defaults to one read |
| `Button` | keeps a caller's `aria-busy` while `busy` is unset |
| `InfoIcon` | the ⓘ glyph |
| `useOxyMode()` (`@oxy-hq/sdk/shell`) | the mode in force (`"light"` / `"dark"`), read from `<html>` and kept current, for what a stylesheet cannot restyle (map tiles) |

Drawn differently for every app: a `Chip` with neither `tone` nor `color` is
`neutral`; the failed-read message is 13px at either size (decision 85); a trap
skips what a closed `<details>` hides; the inset ring's forced-colours outline is
drawn inside; `Field` wires only a labelable control; a `ConfirmDialog` body's
paragraphs lose the browser's margins.

Drawn differently for apps that use them:

- **A `Stat` with a `tip` shows its ⓘ on a desk** (decision 89), so a keyboard reaches
  the source; `tipMark={false}` keeps the hover alone. No tip, or a phone: unchanged.
- **`TrendChart` labels hold 12px on screen at every width** (decision 92): they no
  longer grow with the chart (about 15px in a 694px dialog before). The chart measures
  the width it is drawn at and thins its month labels by it. At 560px and wider it is
  the reference's 560 x 178 drawing; narrower, a desk chart draws 1:1 as `fluid` does.

### Format (`@oxy-hq/sdk/format`)

Figures, dates and lists for a reader: `formatMoney`, `formatNumber`,
`formatPercent`, `formatDate` and its kin, `formatAsOf`, `formatDataThrough`,
`formatAge`, `formatRelative`, `countPhrase`, `plural`, `joinNames`,
`sentenceCase`, `toCsv`. Pure functions with no React, DOM or Node built-ins, so a
Function imports them as readily as a screen. An instant is shown in the `timeZone` you
pass; without one, in the viewer's own zone in a browser and in UTC in a Function, whose
isolate has no `document` and runs on the server's clock (`defaultTimeZone()`, decision
65). Pass the business's zone when a time belongs to a place. A calendar date
(`"2026-11-19"`) is never shifted by a zone. The house rules (the `—` for a
missing value, the minus, whole dollars, the compact bands) are stated once, at the
top of `src/format/index.ts`.

### The theme guard (`checkThemeCss`, `@oxy-hq/sdk/testing`)

An app's test that its own CSS works with the theme rather than against it:

```ts
import { checkThemeCss, formatThemeFindings } from "@oxy-hq/sdk/testing";

test("app CSS never restates an SDK token", () => {
  const found = checkThemeCss(["src/index.css", "index.html"]);
  expect(formatThemeFindings(found)).toBe("");
});
```

Each finding is a `file:line` with a rule: `restates-sdk-token` (a `theme.css` name
declared outside a Tailwind `@theme` block), `literal-colour-in-theme` (a colour
pinned inside `@theme`), `bare-mode-class` (`.dark` / `.light` not keyed on `:root`
or `html`; `:root:not(.dark)` and Tailwind's `@custom-variant dark` are fine),
`own-color-scheme`, and `own-mode-block` (`[data-theme]`, `[data-oxy-mode]`,
`prefers-color-scheme`: the SDK owns light and dark). HTML files are read for their
`<style>` blocks. It reads structurally, so `.btn--primary:focus-visible` is never
taken for a declaration of `--primary`.

Give it file names and let it read them. Vitest hands a test an empty string for a
CSS import, `?raw` included, so `checkThemeCss` fed from
`import.meta.glob("…", { query: "?raw" })` checks nothing; pass the glob's keys (the
paths) instead.

### The no-copy guard (`checkSdkUiCopies`, `@oxy-hq/sdk/testing`)

An app's test that it keeps no copy of a piece `@oxy-hq/sdk/ui` ships:

```ts
import { checkSdkUiCopies, formatSdkUiCopies } from "@oxy-hq/sdk/testing";

test("src/ keeps no copy of an @oxy-hq/sdk/ui piece", () => {
  expect(formatSdkUiCopies(checkSdkUiCopies("src"))).toBe("");
});
```

It reads `src/` (tests, `.d.ts` and `node_modules` left out) and reports every place a
file defines or exports a component under an SDK piece's name (`function SidePanel`,
`const Panel =`, `export { Mine as LoadError }`), with its `file:line`, the import to
use (`use`) and the line that keeps a deliberate variant (`allow`). Importing or
re-exporting the SDK's own is fine. A variant kept on purpose says why, in the file
that defines it; the reason after `--` is required:

```ts
// oxy-ui-local: SidePanel -- docks left as well as right, which the SDK's does not
```

The names (`SDK_UI_PIECES`) are the ui entry's own components, icons and patterns
included, held to its exports by the SDK's own tests.

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
