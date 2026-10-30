---
name: oxy-custom-apps
description: >-
  Use when building, debugging, reviewing, shipping, or verifying an Oxy
  custom-app bundle (a code-first React/Vite app served by Oxy at
  /customer-apps/<org>/<slug>/), including writing Oxy Functions, driving them
  from the UI, and checking a published app in the browser with the Playwright
  MCP server (no CI checks a published bundle). Covers the oxy-app.json
  contract, @oxy-hq/sdk and @oxy-hq/vite-plugin, where app data goes
  (ctx.airhouse, ctx.oltp, ctx.storage), pitfalls, performance guardrails, load
  speed and usage tracking (service worker, Activity tab, useTrackEvent), the
  self-serve oxyc publish ship flow, dev-<handle> sandboxes and workspace
  previews, how an AI agent gets its own short-lived credential (oxyc tokens
  create --sandbox-agent or --agent, never the engineer's oxyc login), and
  reading a deployment's live data with oxyc api and oxyc proxy (which replaced
  oxy api and the Rust oxy login, publish and proxy).
---

# Custom-app development

Custom-app bundles are arbitrary JS apps served by Oxy at
`https://<env>/customer-apps/<org>/<slug>/`. Oxy is a data gateway:
serves static assets, gates cookie auth, and proxies queries. The
bundle is opaque above the data plane — no per-app server config.

This file is the contract. The detail lives in `references/` — see
[Read next](#read-next) for which file answers what.

## The Oxy App Kit (use it)

Two small packages remove the footguns — default to them rather than wiring a
bundle by hand:

- **`@oxy-hq/sdk`** — the data layer (`useQuery`, `useSemanticQuery`,
  `useAgentRun`, `<OxyChat>`); see rule 2 and the `@oxy-hq/sdk` README.
- **`@oxy-hq/vite-plugin`** (`oxyApp()` in `vite.config.ts`) — bakes the asset
  **base path** from the manifest, copies `oxy-app.json` into `out/`, proxies
  `/api` in dev, and injects the `window.__OXY_APP__` shim. Dropping it
  re-introduces the #1 bug — asset 404s → blank dashboard (rule 3).

Scaffold both with `pnpm create @oxy-hq/oxy-app my-app` (templates `vite` — the
default — `functions`, `dashboard`, `single-store`; `-t functions` adds Oxy
Functions). The platform is kit-agnostic (any bundle with a correct base path
works), but hand-rolling means owning the base-path + dev-proxy wiring yourself.

## Three rules

1. **`oxy-app.json` (project root, not `public/`) is identity only.**
   Schema:

   ```json
   {
     "$schema": "https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json",
     "schemaVersion": 2,
     "slug": "url-slug",
     "orgSlug": "your-org",
     "name": "Display Name"
   }
   ```

   Required: `schemaVersion` (must be `2`), `slug` (1–63 lowercase
   letters, digits and single hyphens — it becomes the served path and
   the OLTP schema name).
   Strongly recommended: `orgSlug` (lets `@oxy-hq/vite-plugin` derive
   the asset base path automatically, so plain `pnpm build` produces
   a bundle ready to drop into the linked org with no env var, and lets
   `pnpm dev` and `oxyc publish` find the app without `--org` / `OXY_ORG`;
   `create-oxy-app --org <slug>` writes it).
   Optional: `name` (display), `description`, `icon`, `art`, `status`
   (launcher card), `projectId` (dev-time hint), `build` (what `oxyc
   publish` runs — `install`, `command`, `outDir`; the plugin reads
   `outDir` too), `functions`, `env`, `storage`, `notifications`, `widgets`, …

   **One contract.** The file is validated against the JSON Schema
   `@oxy-hq/sdk` ships (`schema/oxy-app.schema.json`, generated from the
   SDK's `OxyAppManifest` type; `$schema` gives your editor completion)
   by `oxyc validate`, `oxyc publish`, the Vite plugin and the SDK's own
   loader — the same check, the same words. A wrong type, a bad name or
   a missing required field is refused; a field the schema does not know
   is a **warning** (the platform ignores what it does not read — so a
   typo'd `timeoutSecond` is said, not silently dropped). Every field and
   what the server does with it: `OxyAppManifest` in
   `@oxy-hq/sdk/manifest`, which also exports the schema, `isValidSlug`
   / `isValidFunctionName` and `validateOxyAppManifest` for a tool of
   your own.

   At serve time, oxy injects the authoritative org + project from
   the admin row via `window.__OXY_APP__` — the manifest's `orgSlug`
   and `projectId` are advisory hints only, never enforcement.

   Do not invent `products`, `writers`, or `from:`-blocks. Those
   don't exist in v2 and the schema rejects them.

   File location: **`<root>/oxy-app.json`**, sibling to
   `vite.config.ts`. The vite-plugin reads it from there, and so does
   `oxyc publish` (only from the directory it runs in); legacy bundles
   that kept it under `public/` get neither the plugin's base-path
   derivation nor the publish's `slug` / `orgSlug`.

2. **All data flows through SDK hooks → oxy HTTP endpoints.** The
   surface is intentionally small:

   | Hook / component | Endpoint | When |
   | ---- | -------- | ---- |
   | `useQuery({sql})` | `POST /api/projects/:id/query` | Raw SQL, 10k row cap |
   | `useSemanticQuery({topic, …})` | `POST /api/projects/:id/semantic-query` | Reference topics/measures by name |
   | `useAgentRun({agentId})` | `POST /api/projects/:id/agents/:id/asks` + SSE | Streaming chat / Q&A |
   | `useProcedureRun({procedureId})` | `POST /api/projects/:id/procedures/:id/runs` ⚠️ beta | Long-running batch |
   | `useFunction(name)` | `POST /customer-apps/<org>/<slug>/fn/<name>` + SSE | Invoke a server-side **Oxy Function** (see below) |
   | `<OxyChat agentId="…"/>` | wraps `useAgentRun` | Drop-in chat UI |
   | `<OxyAnswer {...run}/>` | renders any run | Drop-in answer renderer |

   ```tsx
   import { OxyChat, useQuery } from "@oxy-hq/sdk";
   const { rows } = useQuery({
     sql: "SELECT col FROM t WHERE x = {{ params.x | sqlquote }}",
   }, { params: { x: someState } });
   // ...
   <OxyChat agentId="analytics" />
   ```

   See the `@oxy-hq/sdk` README for the full reference, including
   `OxyApiError` parsing and beta-surface caveats.

   **Server-side work → Oxy Functions, not the browser.** `useQuery` &
   friends run in the client; for work the browser shouldn't do
   (warehouse writes, ELT kick-offs, external API calls, secret-bearing
   logic) ship a **TypeScript handler under `functions/`** and invoke it
   with `useFunction(name).invoke(body?)`. Declare it in `oxy-app.json`:

   ```json
   { "schemaVersion": 2, "slug": "my-app",
     "functions": { "daily-rollup": { "route": true, "timeoutSeconds": 30 } } }
   ```

   ```ts
   // functions/daily-rollup.ts — runs on Oxy's isolate runtime
   export default async (req, ctx) => {
     const { rows } = await ctx.query("SELECT … FROM …");   // data-plane ctx
     return Response.json({ rows });
   };
   ```

   `ctx` bridges to the data plane: `ctx.query` / `ctx.queryStream`,
   `ctx.semantic.query`, `ctx.warehouse.query`,
   `ctx.warehouse.{insert,exec,upsert}` (to Airhouse, or to a customer
   warehouse the function names in `customerWarehouseWrites`), `ctx.airway.run`,
   `ctx.fetch` (SSRF-allowlisted), `ctx.env`, `ctx.user`, `ctx.log`.
   What the app STORES picks its surface by shape — facts through
   `ctx.airhouse`, records through `ctx.oltp`, files through
   `ctx.storage` — and customer warehouses are read-only. See
   **Where data goes** below before writing anything.
   `oxyc publish` bundles `functions/*.ts` with
   esbuild — no separate backend to stand up. Author-facing types:
   `OxyFunctionContext` / `OxyFunctionRequest` from `@oxy-hq/sdk`; a worked
   example ships in the `functions` template (`pnpm create @oxy-hq/oxy-app
   my-app -t functions`).

3. **`OXY_APP_BASE_PATH` must match the path the bundle is linked
   under.** `@oxy-hq/vite-plugin` resolves it from (in order):
   `OXY_APP_BASE_PATH` env → `orgSlug + slug` in `oxy-app.json` →
   `/`. `oxyc publish` sets that env to `/customer-apps/<org>/<slug>/`
   for the build automatically, and the vite-plugin derives the same
   from the manifest for `pnpm dev` / `pnpm build`. Mismatch → asset
   404s → blank dashboard.

## Where data goes (the shape of the data picks the store)

| The data | Store | In a function | In `oxy-app.json` |
| --- | --- | --- | --- |
| **Facts** — something happened and won't change: an order, a completed checklist, a delivery, a reading | The workspace's Airhouse, in your schema `app_<writer>` | `ctx.airhouse.append(table, rows)` · `ctx.airhouse.query(sql)` · `ctx.airhouse.exec(sql)` | function: `"airhouse": { "enabled": true }` · app: `"airhouseMigrations": { "dir": "airhouse-migrations" }` |
| **Records** your app edits — bookings, assignments, templates, counts | The org's OLTP Postgres, in your schema `app_<writer>` | `ctx.oltp.query/exec(sql, params)` · `ctx.oltp.tx(async (tx) => …)` | function: `"oltp": { "enabled": true }` · app: `"migrations": { "dir": "migrations" }` |
| **Files** — photos, PDFs, exports | Your app's storage silo | `ctx.storage` | function: `"storage": { "read": true, "write": true }` |
| **The customer's warehouse** — ClickHouse, Snowflake, BigQuery, their Postgres | Theirs | `ctx.query` · `ctx.warehouse.query(db, sql)` | read-only: a write is refused unless the function declares `"customerWarehouseWrites": { "<db>": "<why>" }` |

Three questions, in order: **Is it bytes?** → storage (keep the key in a record
or fact). **Does it change after it's written?** No → a fact; yes → a record;
both when you need history (the record says what is true now, the fact says
what changed). **Is it already in the customer's warehouse?** → read it there.

`<writer>` is your slug with `-` → `_` (`my-app` → `app_my_app`); a slug
with `_` can't back a schema. Both stores are written **as the app**, whoever
invoked the function, so a scheduled run writes like a click.

**Facts, in practice.** Airhouse is DuckLake: no `PRIMARY KEY`, `UNIQUE`,
indexes or foreign keys — publish refuses a migration carrying one. So:

- Give every fact the id its source assigned and a `recorded_at`. A retried
  function appends twice; readers keep one row per id:
  `QUALIFY row_number() OVER (PARTITION BY completion_id ORDER BY recorded_at DESC) = 1`.
- A correction is a new fact (`voided`, `amended`), not an `UPDATE`.
- Name tables `app_<writer>.<table>` in migrations and in `exec`/`query`
  (`ctx.airhouse.schema` has the name). `append` takes the bare table name.
- One statement per call, nothing bound: request data goes through `append`,
  never into `exec` SQL. `exec` runs no DDL — tables come from migrations.

**Records, in practice.** Bound parameters (`$1`, `$2`) always. Several
statements that must land together go in one `ctx.oltp.tx`. The store is
provisioned per org by an operator; until then calls fail naming that step.

**Not a store:** `ctx.secrets` holds credentials you rotate. A cursor, a
counter or a JSON blob is a record. `oxyc validate` warns about it, about
keys in Airhouse migrations, and lists every customer-warehouse write exception.

```ts
// A checklist is finished: the record changes, and a fact is appended.
async function completeChecklist(ctx, runId: string, storeGuid: string) {
  await ctx.oltp.tx(async (tx) => {
    await tx.exec("UPDATE checklist_runs SET status = 'done', completed_at = now() WHERE id = $1", [runId]);
    await tx.exec("DELETE FROM open_reminders WHERE run_id = $1", [runId]);
  });
  await ctx.airhouse.append("checklist_completions", [
    { completion_id: runId, store_guid: storeGuid, completed_by: ctx.user.id, recorded_at: new Date().toISOString() },
  ]);
}
```

## Telling people something

Name **people**; the platform reaches them. Don't build a recipient table, a
"sent" log or a Slack webhook helper:

```ts
await ctx.notify({
  to: { role: "Store Manager", location: storeId }, // or { userId } · { team }
  kind: "list_overdue",                             // declared in oxy-app.json
  title: "Opening list is 30 minutes late",
  link: `/lists/${listId}`,                         // a path inside the app
  email: true,                                      // optional; slack: true too
  idempotencyKey: `overdue:${occurrenceId}`,        // retry-safe
});
```

`oxy-app.json`: `"notify": { "kinds": { "list_overdue": {} } }` on the function
(`{ "critical": true }` for a kind people must not mute). Everyone lands in
their Oxygen inbox — frontline crew too, who have no email — and is pushed,
emailed or Slacked where they can be. `slack: true` DMs each person's own Slack
account (found by their email once the org has connected Slack), so an app
never holds a Slack token or maps users to Slack ids. Needs the org's OLTP
store. Use
`ctx.email.send` only for addresses **outside** the org (a vendor, an
accountant), and give it an `idempotencyKey` in a scheduled function.

### Categories, levels, threads and actions

Describe your kinds once, at the top level of `oxy-app.json`. It is the tree
people mute in (a whole app, a category, or one kind) and the one a phone groups
by, so declare it with the first `ctx.notify`, not later:

```json
"notifications": {
  "categories": {
    "operations": { "label": "Store operations" },
    "operations/food-safety": { "label": "Food safety" }
  },
  "kinds": {
    "list_overdue": { "category": "operations", "label": "Overdue lists", "level": "time_sensitive" },
    "temp_alarm": { "category": "operations/food-safety", "label": "Temperature alarm", "level": "critical",
                    "actions": [{ "id": "ack", "label": "Acknowledge", "function": "ack-alarm" }] }
  }
}
```

- **Category**: a path of up to 3 segments (`a-z0-9`, `_`, `-`); a declared
  path's parents need no entry. A kind with none is in `general`.
- **Level**: `passive`, `active` (default), `time_sensitive`, `critical`. Only
  `critical` ignores mutes; a function's `"critical": true` still makes it so.
- **Actions**: at most 3, label ≤ 24 characters. With `function` (one this app
  declares), a tap calls it as the person with `{ notificationId, action, link }`
  and marks it read; without, it opens the link.
- **Thread**: ``ctx.notify({ …, thread: `store:${storeId}` })`` (≤ 128
  characters) folds a run of notifications about one thing into the newest.
- The function's `notify.kinds` still decides what it may send; a kind it sends
  that the catalogue leaves out gets the defaults. `oxyc validate`, the Vite
  plugin (`pnpm dev` / `pnpm build`) and `oxyc publish` refuse a malformed or
  unknown category, a bad level, more than 3 actions, an action naming an
  undeclared function, a misspelt field and any `oxy.` kind — every problem at
  once; a catalogue kind no function sends is a warning.

**The dock.** Wrap the app in `<OxyShell>` (every `create-oxy-app` template
does) so it shows Oxygen's rail: HQ, Chat and the viewer's other apps, each
opening in this tab and warmed on hover. Pass `hideTopBar` if the app draws its
own header. Link to another app with a plain `<a href>`, never `target="_blank"`;
moving between apps is meant to feel like one product.

```tsx
import { OxyShell } from "@oxy-hq/sdk/shell";
import "@oxy-hq/sdk/shell.css";

<OxyAppProvider>
  <OxyShell hideTopBar>
    <App />
  </OxyShell>
</OxyAppProvider>
```

A `pushState` before the person has touched the page (a default redirect,
filters written to the query string on mount) is turned into a replace by the
platform, so Back still leaves the app; write such URL syncs as replaces anyway. An app that pushes on
purpose before any interaction (a dialog route opened on load and closed with
`history.back()`, a kiosk's Back trap) adds
`<meta name="oxy-history" content="native">` to its `index.html`, and its pushes
are left alone.

The app is its own page, outside Oxygen's shell, so show the inbox **in the
app** — people who live in it (crew on a kiosk) never see Oxygen's bell:

```tsx
import { NotificationsBell } from "@oxy-hq/sdk";

<NotificationsBell onNavigate={(link) => navigate(link)} /> // in the header
```

It lists this app's notifications (`scope="all"` for the whole inbox), each
with its category under the title, `critical` and `time_sensitive` ones marked,
and threads folded; a tap
hands the `link` you passed to `ctx.notify` to `onNavigate` (without it, the
page loads that path under the app's base). It renders nothing where there is
no inbox (signed out, an older server, `pnpm dev`). `useNotifications()` gives
`{ items, unread, markRead, markAllRead, refresh, … }` to draw your own, and
takes `{ category, kind, thread }` to narrow it. Theme
it with `--oxy-notifications-*` CSS variables. Push sign-up and mutes stay in
Oxygen's Settings → Notifications — don't build them. In tests,
`createTestInbox({ manifest })` from `@oxy-hq/sdk/testing` is a fake inbox to pass as
`<OxyAppProvider fetcher={inbox.fetcher}>`.

## Home widgets (a live card on the Oxygen home)

A widget puts what needs doing in your app on the Oxygen home, with the button
that does it ("19 stores ready to post" and **Post 19 ready**). It is **data, not
a page**: a function of yours returns a small JSON view and the home draws it
with its own components. No app code runs on the home, and the function runs as
the person looking, so app access, rate limits and capability gates all apply.

```json
"widgets": {
  "close": {
    "title": "Month-end close",          // ≤ 40 characters
    "function": "close-widget",          // declared under functions; returns the view
    "size": "wide",                      // "regular" (default) or "wide"
    "actions": {
      "post-ready": { "function": "post-ready-jes", "confirm": true }  // destructive: true for red
    }
  }
}
```

At most **3 widgets** per app and **3 actions** per widget; ids are lowercase
letters, digits, `-` and `_`. `oxyc validate` and `oxyc publish` refuse an unknown
key, a bad id, and a `function` the manifest does not declare.

The widget function is called with `{ widget: "<id>" }` and returns a `WidgetView`:

```ts
import type { WidgetView } from "@oxy-hq/sdk";

export default async (req, ctx) => {
  const view: WidgetView = {
    headline: "19 stores ready to post",   // required, ≤ 80; none = "nothing to show"
    detail: "September · 61 of 84 posted", // ≤ 120
    level: "attention",                    // ok | info | attention | critical
    progress: { done: 61, total: 84 },
    rows: [{ title: "Valencia", detail: "Not mapped", state: "blocked", link: "/settings" }], // ≤ 6
    buttons: [{                            // ≤ 2, each naming a declared action
      action: "post-ready", label: "Post 19 ready", input: { period: "2026-09" },
      key: "2026-09", confirm: "Post 19 journal entries to QuickBooks?"
    }],
    link: "/?month=2026-09",               // "Open": app-relative, starts with / or ?
    badge: 19,
  };
  return Response.json(view);
};
```

Text past its limit is cut, lists past theirs are cut, a link that is not
app-relative is dropped, and a button naming an action the live build does not
declare is dropped (and refused if pressed).

**Pressing a button** calls the action's function with
`{ widget, action, input }` and an `Idempotency-Key` built from widget, action
and the button's `key` (no `key`: a hash of `input`). A double click, a second
tab, or a stale card pressed after the work was done replays the first answer, so
**give every button a `key` that names the thing done** (`"2026-09"`), not a
timestamp. What the action returns decides what the card shows: `{ view }`
re-renders at once, `{ message }` shows that line, anything else makes the home
read the widget again. A throw or non-2xx leaves the card as it was, with the
reason under the button.

Widgets show **in production only**, and only on the **apps-first home** (the
feature flag that turns that home on). A staging or sandbox host is not read for
them. A widget that should not run on every home load declares
`cache: { ttlSeconds }` on its function; the cache is per person and build.

## Four pitfalls

- **CORS errors are usually wrong-URL errors in disguise.** The SDK's
  base URL must be `${OXY_URL}/api`, not `OXY_URL` bare. The API
  router (which carries CORS headers) lives under `/api/*`. Hitting
  `/<projectId>/...` lands in the static fallback and the missing
  headers look like a CORS failure.

- **Cookie auth requires `credentials: "include"`.** The SDK's
  `useQuery` sets this. If you fetch directly, set it yourself or the
  `oxy_session` cookie won't be forwarded.

- **`projectId` in the manifest is advisory, not enforcement.** The
  SDK uses it to construct the query URL. Authorization is the user's
  cookie → user → org_member of the project's org, decided
  server-side. A bundle that ships a stolen `projectId` can't read
  data the user can't already read.

- **Cloud mode is the target.** Don't branch on local vs. cloud; don't
  rely on the nil UUID `00000000-...` as a default project; don't
  assume `OXY_STATE_DIR` is writable.

Two more that read as bugs — `useTrackEvent("oxy-…")` is refused (`400`,
the prefix is reserved), and a hovered-but-never-opened app records no view —
are in [references/performance-and-usage.md](references/performance-and-usage.md).

## Run a function locally

`oxyc fn dev <name>` runs one handler from your source in-process, in seconds,
with no publish. It bundles the handler with the same esbuild call `oxyc publish`
uses, then invokes it on `createTestContext` seeded from `oxy-app.json`: the
function's capabilities, `ctx.env` from the env file, and its destinations. It
prints the response, then the host ops the handler made.

```bash
oxyc fn dev notify --input '{"name":"Ada"}'            # a literal, @file, or - for stdin
oxyc fn dev notify --input @sample.json --env-file .env.dev --watch
oxyc fn dev report --read-through --env dev     # reads hit that deployment; writes stay stubbed
```

- **Run it from the app directory** (the one with `oxy-app.json`). `--env-file`
  defaults to `.env`; `ctx.env` holds that file's keys and nothing from your
  shell. A `required` secret the file lacks is a warning.
- **`--watch` re-runs** when a file in the app changes, reading the manifest,
  env file and `@file` input afresh. A broken edit prints its error and waits
  for the next save. Exit with Ctrl-C.
- **Exit codes:** `0` for a 2xx `Response`; `1` when the handler throws or
  answers non-2xx (as `fn call`); `2` for a function the manifest does not
  declare, or a missing env file.
- **A local run is not the isolate.** There is no timeout, memory or log cap;
  `Buffer`, `TextEncoder` and `process` exist where the isolate lacks them; and
  every warehouse, semantic, storage, fetch and email call is a stub in memory,
  so (without `--read-through`) nothing reaches a deployment. The context does refuse what the manifest
  does not grant, in the host's words. A `ctx.warehouse` destination's engine is
  assumed (a `customerWarehouseWrites` entry is a customer Postgres, any other
  is Airhouse), because the workspace config is not read.
- **`--read-through` reads real data, and only reads.** It sends `ctx.query`,
  `ctx.warehouse.query` and `ctx.semantic.query` to the deployment `--env <name|url>`
  (or `--target`) names, through the app's own `POST /api/projects/{project}/query`
  and `/semantic-query`, with your `oxyc login` bearer or `OXY_TOKEN` (the way
  `oxyc api` authenticates). What you may read is the server's decision. The
  project is `--project` / `OXY_PROJECT`, else looked up from `oxy-app.json`
  `slug` + `orgSlug`. Every other call (warehouse writes, `ctx.tx`, `ctx.oltp`,
  `ctx.airhouse`, storage, fetch, email, notify, and `ctx.queryStream`) stays a
  stub in memory and never leaves your machine. Each call in the report is marked
  `real` or `stubbed`. `--env` defaults to production, so name a dev environment
  when you can. A sandbox agent token is refused: use a person's login.
  `ctx.semantic.query` row keys are `view__member` in the isolate but the route
  strips the prefix, so read-through puts it back from your spec; a key it could
  not map unambiguously stays as the route named it, so confirm that one with
  `fn call` on a sandbox. Still not the isolate.
- Before you ship, run it where it really runs: publish to your sandbox and
  `oxyc fn call`, or `oxyc checks run`. For a committed regression test use
  `createTestContext` from `@oxy-hq/sdk/testing` in vitest, as the functions
  template does.

## Publishing

**These commands are a person's, or an agent's on a token that person
approved.** `oxyc login` is the engineer's own browser login, and a publish to
a channel or a promote is refused under a sandbox agent token. As an agent,
iterate in your own sandbox
([references/sandbox-agent-loop.md](references/sandbox-agent-loop.md)). Ship
only when your engineer asked you to, on an agent token they approved
(`oxyc tokens create --agent`, plus `--standing` for staff access): it
publishes and promotes as them. With no browser on your machine, see
[Headless and cloud sessions](#headless-and-cloud-sessions): a token approved
there never carries staff access.

Ship with `oxyc publish` (`npm i -g @oxy-hq/cli`) — **no CI step, no S3 sync,
no folder picker**. From the app directory:

```bash
oxyc login --env production    # once; browser flow, caches a token,
                               # and prints whether you're an app-admin
oxyc validate                           # schema, data-placement and function checks first
oxyc publish --env production           # build → bundle functions → tar → upload (draft = staging)
oxyc promote <org>/<app> --env production --dry-run   # the plan: changes, migrations, blockers
oxyc promote <org>/<app> --env production             # staging's build → production (asks first)
oxyc rollback <org>/<app> --env production --app-env production   # back to the build before
```

Production only ever serves a build staging has served, and every way of
moving it — `oxyc promote`, `oxyc rollback`, the console's **Promote draft**
and **Roll back to…**, a promoting publish — applies the build's pending migrations **before** it moves
(a failure leaves the old build live). `promote` and `rollback` print the plan
first and refuse on a blocker: `staging_never_served`, `required_secrets_unset`
(set the key in production first), `schedule_removal_unconfirmed` (pass
`--remove-schedule <function>`). Off a terminal pass `--yes`.
`oxyc publish --promote` still works and warns that it is deprecated: it shows
nobody the plan.

- **Identity**, for each half, is `--app` / `--org`, then `OXY_APP` /
  `OXY_ORG`, then the manifest's `slug` + `orgSlug`, then an
  `apps/<org>/<app>/` working directory. `--org` takes a slug or a UUID; pass
  the slug whenever `oxyc` runs the build, because the asset base path is
  baked from what you pass. With no org from any of those, the build stops
  with `missing org: the app's base path needs it` (exit `2`): add
  `"orgSlug"` to `oxy-app.json` (`create-oxy-app --org <slug>` writes it)
  or `--org <org-slug>`, and `--app <slug>` when the directory has no
  manifest. The server upserts the app row — there is no separate create
  step.
- **First publish of a new app is one step.** The app is not registered
  yet, so the build-config lookup 404s; with your login `oxyc publish` then
  lists the org's workspaces, and when there is exactly one (a freshly
  provisioned org) publishes into it and says which. With several it prints
  them with their uuids beside the flag to pass — `--project
  <workspace-uuid>` (or `OXY_PROJECT`), which also supplies the org. Drop
  the flag after the first publish — it's auto-resolved from then on.
  Trusted publishing (CI) only ever publishes a registered app.
- **Old kit.** Before the build, an app whose `@oxy-hq/sdk` or
  `@oxy-hq/vite-plugin` is more than one minor behind the versions this
  `oxyc` was built with gets one warning naming the `pnpm add` — the
  manifest contract and the function runtime move with them.
- **`--env` targets:** `local` → a local Oxy web-app Vite dev server `:5173`
  (it serves `/cli-auth` and proxies `/api` to oxy on `:3000`), `dev` →
  `aip.dev.oxy.tech`, `staging` → `aip.staging.oxy.tech`, `production` (the
  default) → `app.oxygen-hq.com`. A value with a `.`, `/` or `:` is read as a
  URL.
- **Function lint.** Before the build, each Oxy Function's source is checked
  for what the host would refuse at the first call: a `ctx.*` call whose
  capability the manifest lacks, a global the isolate does not have
  (`Buffer`, `TextEncoder`, `process`, …), a write outside `destinations`. A finding fails
  the publish (exit `1`); `--allow-function-lint` publishes past a false
  positive.
- `--dir out` skips the build and uploads a prebuilt directory (declared
  functions are still bundled into it); `--build-only` builds and bundles
  without a credential or an upload; `--prebuilt` (with `--dir`) uploads a
  directory whose functions are already bundled. Server warnings print on
  stderr; `--json` prints the result.
- `.env.local` / `.env` are loaded from the app directory and its parents
  (never overriding what is already set), so a stray `OXY_PROJECT` there
  retargets a publish.
- **Credential:** `OXY_TOKEN`, then `oxyc login`'s cache — or, in a GitHub
  Actions job with `id-token: write`, the job's OIDC identity.
  `oxyc init-ci` writes a workflow that publishes with no stored secret.
- Promote / roll back / unpublish from the oxy admin app-detail console;
  every publish is audited (who shipped each build, when).
- **What is live now:** `oxyc apps show <org>/<app>` prints the live build, the
  commit it was built from, a newer draft if there is one, and its health;
  `oxyc apps builds <org>/<app>` is the history. `oxyc apps drift <org>/<app>`
  lists the commits in your checkout that touch the app after the live build's
  commit — `unknown` there means it could not compare, not that nothing changed.
- **A customer's own domain** (`app.acme.com`) serves production only:
  `oxyc apps domains add <org>/<app> app.acme.com` prints a CNAME and a TXT
  record for the customer to create, then `oxyc apps domains verify …`. Build
  nothing differently — the bundle keeps its `/customer-apps/<org>/<app>/` base
  and its same-origin `/api` calls work on the domain. A visitor signs in once
  through the Oxygen login; on that domain the session reaches only the app's
  workspace, so a call to another workspace there is refused by design.

The build is not the check: after a publish, drive the page
([references/verifying-in-the-browser.md](references/verifying-in-the-browser.md)).

## Headless and cloud sessions

A Claude Code cloud session (claude.ai/code), a CI runner or any machine with
no browser can mint a token, iterate in a sandbox and publish. A person
approves once, from any device.

1. **The cloud environment.** Its owner sets this once in the environment's
   settings. Under **Network access → Custom**, allow `app.oxygen-hq.com` and
   `*.oxy.tech`, and keep the default package-manager list ticked so npm can
   install `oxyc`. Optionally add `OXY_APPROVER=<their email>` under
   environment variables, so each approval request is pushed to them.
2. **The repo's SessionStart hook,** `.claude/hooks/oxyc-cloud-session.sh`,
   with its entry in `.claude/settings.json`. In a cloud session it installs
   `oxyc` and routes it through the session's proxy. It also makes a minted
   token reach every later command, which an `export` in one shell call cannot
   do. What it prints at session start says what is missing.
   - A customer repo that `oxyc new` or `oxyc adopt` made has both files.
   - In an existing customer repo, `oxyc update --apply` adds the script but
     only reports the settings entry, because the team may have their own
     settings. Add the entry by hand:
     ```json
     { "hooks": { "SessionStart": [ { "matcher": "", "hooks": [ { "type": "command",
       "command": "\"${CLAUDE_PROJECT_DIR:-.}/.claude/hooks/oxyc-cloud-session.sh\"",
       "timeout": 120 } ] } ] } }
     ```
   - Any other repo can copy both from the `template/.claude/` directory of the
     installed `@oxy-hq/cli` package.
3. **The credential.** Run this in the background, because the approval can
   outlast a two-minute shell limit:
   ```bash
   mkdir -p ~/.config/oxy && (umask 077; oxyc tokens create --agent --no-browser \
     --env production >~/.config/oxy/agent-token.env)
   ```
   Give your engineer the URL and the code it prints on stderr. The token goes
   to that file and never into your context. The hook loads the file before
   every later command. To iterate in a sandbox, mint
   `--sandbox-agent --app <org>/<app>` into the same file instead. A token
   lasts 8 hours by default (`--hours`, up to 168). On exit `4`, mint again.
4. **Ship** with the commands above, each with `--token-env OXY_TOKEN`.
   `promote` and `rollback` need `--yes`, because there is no terminal to ask.

**A token approved by device code never carries staff or partner access.**
`--standing --no-browser` is a usage error (exit `2`). The token from step 3
reaches only what the approver's org memberships do. That is enough when they
belong to the app's org. Staff shipping a customer's app have three ways:

- **Let the repo's CI publish a draft, and a person promote it.** This works
  only where the repo turned CI publishing on, with the repository variable
  `OXY_CI_PUBLISH=on`. Without it, the publish job is skipped and the run
  still shows green.
  - `gh workflow run publish.yaml -f ref=<branch>` publishes that ref as
    production's draft, which is staging. A push to `main` publishes to `dev`
    instead.
  - Check that the publish job ran, not that the run is green.
  - Then your engineer runs `oxyc promote <org>/<app> --dry-run`, then
    `oxyc promote <org>/<app>`, and sees the plan before it ships.
  - Never pass `-f promote=true`. It rebuilds the ref and puts it live with no
    plan, the deprecated promoting publish.
- **Mint the standing token where a browser opens.** On the engineer's own
  machine, run `oxyc tokens create --agent --standing --hours <n>`. They put
  the value in the cloud environment's variables as `OXY_TOKEN`. Anyone who
  uses that environment can read it, so keep `--hours` short and revoke the
  token when the task is done.
- **Leave the ship step to the engineer.** Hand back the sandbox's build and
  the `oxyc promote --from dev-<handle>` command.

Without the hook, start each command with `. ~/.config/oxy/agent-token.env &&`.
If a command fails with `fetch failed` (exit `7`) while `HTTPS_PROXY` is set,
`oxyc` is too old to use the proxy. Update it, or set `NODE_USE_ENV_PROXY=1`.

## Sandboxes — try a change on real data before shipping it

A sandbox (`dev-<handle>`, 1-12 lowercase letters/digits/hyphens, no leading,
trailing or double hyphen) is a named, short-lived environment of one app: its
own build pointer, storage silo, secrets and Airhouse sibling schema. Reads
hit production; most writes are isolated or *held* (recorded, not performed —
`ctx.oltp`/`ctx.warehouse` stay shared; a held write's `note` field in
`invocations held` names the manifest key that would make it land instead).
A sandbox's schedules are **off by default** (staging's too): staff turn them
on per environment with `oxyc env schedules <app> dev-x --on`, and a scheduled
fire is not activity. To test a scheduled function without that, run it once
as its cron would — `oxyc fn run <app> <fn> --app-env dev-x --as-schedule` (no
input, system identity). Webhooks reach a sandbox on their own route,
`/api/webhooks/apps-env/dev-x/<org>/<app>/<fn>`, verified with the sandbox's
own `secretVar` value — set it with `env secret set`; staging's does not count.
It starts with no build, costs nothing idle, and is gone after 7 idle days or
an explicit delete — safe to use for every iteration that would otherwise
touch staging. Limit: 20 per app, counting ones still being deleted.

```bash
oxyc env create acme/store dev-x --env dev --json          # starts with no build
oxyc publish --env dev --app-env dev-x --json               # build + publish to it
oxyc env secret set acme/store KEY --app-env dev-x --env dev --value-env VAR  # optional: its own secret
oxyc fn call acme/store submit-order --app-env dev-x --env dev --data '{}' --json   # call it
oxyc checks run acme/store --app-env dev-x --env dev --json # run its checks
oxyc fn run acme/store nightly-sync --app-env dev-x --as-schedule --env dev --json  # a scheduled function, once
oxyc invocations list acme/store --app-env dev-x --env dev --json  # what ran; a row's id is `id`
oxyc invocations held acme/store <invocation-id> --env dev --json  # what it held, not wrote
oxyc logs acme/store --app-env dev-x --env dev --json       # its function logs
oxyc logs acme/store --app-env dev-x --env dev --follow --timeout 300 --json  # live, as printed
oxyc env delete acme/store dev-x --env dev --yes --wait --json     # done; tears its homes down
```

(`--data` has no short form — don't write `-d`.)

Iterate from `publish` as the code changes — each publish is a new build, and
the sandbox always serves the newest. A sandbox build reaches production
only through staging: a person runs `oxyc promote <app> --from dev-<handle>`
(the build you verified becomes staging's), checks it there, then `oxyc
promote <app>`.
Staff only (needs `develop_apps` + `manage_apps`) — or a sandbox agent token
such staff approved; a publish token is refused on every sandbox operation,
so CI cannot use one.

**An agent runs this loop on its own sandbox agent token**, never a person's
login, with `--token-env OXY_TOKEN` on every command — the mint, exit codes,
rules and what to hand back are in
[references/sandbox-agent-loop.md](references/sandbox-agent-loop.md). Read it
before an agent runs any `oxyc` command against a deployment.

**A sandbox isolates the app, not the workspace.** The semantic model,
automations and Airway pipelines it reads are `main`'s. To try a workspace
branch, open a workspace preview (staff only, `oxyc preview …`), or publish to
the sandbox with `--semantic-branch <branch>` — see
[references/sandbox-agent-loop.md](references/sandbox-agent-loop.md#a-sandbox-isolates-the-app-not-the-workspace).

## Read next

| File | Read it when |
| --- | --- |
| [references/sandbox-agent-loop.md](references/sandbox-agent-loop.md) | You are an AI agent about to touch a deployment: minting the sandbox agent token, the loop, exit codes, rules, what to hand back, when you cannot get a token, the MCP tools, workspace previews. |
| [references/verifying-in-the-browser.md](references/verifying-in-the-browser.md) | You changed something a person sees (`src/**`, or a function a card calls) and need to prove it on the published page. |
| [references/reading-data.md](references/reading-data.md) | You need a deployment's live data while building: an agent's own `--agent` token, `oxyc api` / `routes` / `schema`, and `oxyc proxy` for `pnpm dev` against cloud data. |
| [references/performance-and-usage.md](references/performance-and-usage.md) | You write an Oxy Function, a bulk action, caching or idempotency keys — or need to know what Oxy does for load speed and usage tracking, and the opt-outs. |
| [references/minimal-starter.md](references/minimal-starter.md) | You are writing an app by hand instead of scaffolding it, or need the smallest correct layout. |
