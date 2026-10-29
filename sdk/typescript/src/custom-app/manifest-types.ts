// THE `oxy-app.json` CONTRACT — the one definition every tool derives from.
//
// `schema/oxy-app.schema.json` is generated from `OxyAppManifest` below
// (`scripts/gen-manifest-schema.mjs`; `manifest-schema.test.ts` fails when the
// checked-in copy is stale). `@oxy-hq/sdk/manifest` ships the schema and a
// validator over it, which `oxyc validate`, `oxyc publish`, `@oxy-hq/vite-plugin`
// and this SDK's own loader all run — so a field added here is validated
// everywhere in one edit, and a field the server honours that is missing here
// is the drift this file exists to end.
//
// Doc comments double as the schema's `description`s, and the JSDoc tags
// (`@asType integer`, `@minimum`, `@maximum`, `@pattern`, `@minLength`) are
// read by the generator as constraints — keep them beside the field they bind.
//
// No imports beyond the notification catalogue and the widgets file (also pure types), and nothing
// past ES2020: this module is compiled under every package's tsconfig.

import type { OxyAppNotificationsManifest } from "./notification-catalog";
import type { OxyAppWidget } from "./widgets";

/**
 * Declaration of a single Oxy Function shipped in the bundle's
 * `functions/` dir. See `internal-docs/customer-apps-functions.md`.
 *
 * All fields optional except that at least one invocation surface
 * (`route`, `schedule`, `airwayStep` or `webhook`) must be active. Absent =
 * `route: true` (HTTP-invocable via `useFunction`).
 */
export interface OxyAppFunctionManifest {
  /**
   * Source entry, relative to the app dir. Default: `functions/<name>.ts`.
   * @minLength 1
   */
  entry?: string;
  /**
   * Cron expression. When set, the function fires on this schedule.
   * @minLength 1
   */
  schedule?: string;
  /** IANA timezone for `schedule`. Default: `UTC`. */
  timezone?: string;
  /** Expose `POST .../fn/<name>` (called via `useFunction`). Default: true. */
  route?: boolean;
  /** Wire the function in as an Airway pipeline transform step. */
  airwayStep?: OxyAppAirwayStepManifest;
  /**
   * Receive an unauthenticated POST from a third party at
   * `POST /api/webhooks/apps/<org>/<app>/<name>`.
   *
   * The PLATFORM verifies every request — HMAC-SHA256 over the raw body, in
   * constant time — before the function is enqueued, so app code never sees an
   * unverified request. Omit the block and that endpoint answers 404, as if it
   * did not exist.
   */
  webhook?: OxyAppWebhookManifest;
  /**
   * Wall-clock timeout in seconds. Default 30, max 300.
   * @asType integer
   * @minimum 1
   * @maximum 300
   */
  timeoutSeconds?: number;
  /**
   * Route invocations per minute the host admits for this function before
   * answering 429, per (build, function, caller). Default 60 — the server's
   * `rate_limit::DEFAULT_RATE_PER_MIN`. Raise it for a function a busy UI
   * polls; lower it for one that fans out to a paid API.
   * @asType integer
   * @minimum 1
   */
  rateLimitPerMinute?: number;
  /**
   * `ctx.fetch` limits for this function.
   *
   * The host reads a response fully into memory before handing it to the
   * isolate (×1.33 when base64), on a process shared by every app on the
   * instance — so the 10 MiB default ceiling is raised per function, here,
   * never globally.
   */
  fetch?: OxyAppFetchManifest;
  /**
   * Mark this function as a check. `oxyc checks run` runs it; with a
   * `schedule` the platform runs it continuously and a failure pages. A
   * check fails when it throws, times out, or returns `{ ok: false }`.
   * Checks must not have customer-visible side effects. Outside production
   * only a function its build marks `check: true` may run at all.
   */
  check?: boolean;
  /**
   * Opt-in result caching for route invocations. Omit (the default) to never
   * cache — the safe choice for a side-effectful function (writes, external
   * POSTs, ELT). Set `ttlSeconds` ONLY for read-only / idempotent functions:
   * results are then cached per (build, function, user, request body) for that
   * window, and a repeat `useFunction().invoke(sameBody)` returns the cached
   * result without re-running. A `?refresh` query bypasses it.
   */
  cache?: OxyAppCacheManifest;
  /**
   * Databases this function's `ctx.warehouse.*` writes may target. Omit (or
   * leave empty) and the function may NOT write to any database — writes are
   * fail-closed and rejected before any connection is opened. Declare a
   * destination here ONLY for a function that legitimately writes to it; a
   * read-only function omits it. This scopes writes away from the project's
   * source warehouse.
   *
   * A customer warehouse (anything but `airhouse` / `airhouse_managed`) is
   * read-only: listing it here is not enough, it must also appear in
   * `customerWarehouseWrites` with a reason.
   */
  destinations?: string[];
  /**
   * Customer warehouses this function writes to anyway, each with the reason —
   * the exception to "customer warehouses are read-only". Every key must also be
   * in `destinations`; a write to a customer warehouse not named here is refused.
   * Prefer moving the data: facts to `airhouse`, records to `oltp`.
   *
   * ```json
   * "customerWarehouseWrites": {
   *   "clickhouse": "Journal entries stay beside the legacy ClickHouse facts until QuickBooks lands in Airhouse"
   * }
   * ```
   */
  customerWarehouseWrites?: Record<string, string>;
  /**
   * Capability to write app-scoped secrets via `ctx.secrets.set` (fail-closed:
   * omit → writes rejected). Only the app's own `apps/<app-id>/` namespace is
   * writable. Declare for a function that rotates a credential — e.g. a
   * scheduled token-refresher that writes the rotated token back to Oxy Secrets.
   * Secrets are not a state store: a cursor or counter is a record for `oltp`.
   */
  secrets?: OxyAppSecretsCapability;
  /**
   * Capability to send email via `ctx.email.send` (fail-closed: omit → the
   * host rejects `ctx.email.send` before any provider call). Declare for a
   * function that emails the app's users — e.g. a `notify` route that sends a
   * welcome message, or a scheduled digest. The sender mailbox is
   * platform-controlled; a function may set `replyTo` but never `from`.
   */
  email?: OxyAppEmailCapability;
  /**
   * Capability for `ctx.notify` — tell people in this org something, by user,
   * role at a location, or team (fail-closed: omit → every call rejected).
   * Declare each `kind` the function sends: a kind is what a person mutes, and
   * one marked `critical: true` (a safety alert) ignores their mutes. A call
   * with an undeclared kind is refused. How a kind is labelled, grouped and
   * ranked is the app-level {@link OxyAppManifest.notifications} catalogue.
   *
   * ```json
   * "notify": { "kinds": { "list_overdue": {}, "safety_alert": { "critical": true } } }
   * ```
   */
  notify?: OxyAppNotifyCapability;
  /**
   * Capability for `ctx.storage` (fail-closed: omit → every storage op
   * rejected). `read` permits `getDownloadUrl` / `list` / `get`; `write`
   * permits `getUploadUrl` / `put`. Scoped to the app's own
   * `customer-app-storage/<app-id>/` silo. Declare for a function that accepts
   * uploads or serves stored files. The silo's retention is the app-level
   * {@link OxyAppManifest.storage} block.
   */
  storage?: OxyAppStorageCapability;
  /**
   * Capability for `ctx.org.people()` — the org's people directory, READ-ONLY
   * (fail-closed: omit → the call is rejected before any query).
   *
   * Declare it for a function that has to name a person: an assignee, a roster
   * entry, who submitted something. It answers with a display name and a role.
   *
   * Three things it deliberately is not, so nobody plans around them:
   * it returns **no email and no phone** — naming a colleague is a different
   * need from contacting them off-platform (to reach someone, `ctx.notify`
   * them by id); and it returns **no location**, which the platform does not
   * hold for a member. It does include frontline workers granted to this app,
   * tagged `kind: "frontline"`.
   *
   * One flag, not `read`/`write`: there is no write. Editing the directory
   * would put tenant membership behind an app's manifest.
   */
  org?: OxyAppOrgCapability;
  /**
   * Capability for `ctx.platform.facts()` — ids and counts about every org on
   * the deployment, for an internal app (fail-closed: omit → the call is
   * rejected). Necessary, not sufficient: the host also requires the app's org
   * to be one of the deployment's internal orgs (`OXY_INTERNAL_ORGS`), so
   * declaring it in any other org's app opens nothing.
   */
  platform?: OxyAppPlatformCapability;
  /**
   * Capability for `ctx.oltp` — read/write the app's OWN per-org OLTP schema on
   * the managed Postgres tenant (fail-closed: omit → every `ctx.oltp` call
   * rejected). A pure GATE: the target schema is derived from the app's own slug
   * host-side (`oltp-bookings` → `app_oltp_bookings`), never named here, so a
   * manifest cannot point `ctx.oltp` at another app's schema. The resolved role
   * has DML rights on that one schema and nothing else — reaching neither another
   * app's data nor the analyst-visible `raw_*` schemas. The store must be
   * provisioned first (ask whoever operates the org).
   *
   * NOTE: this shape is `{ enabled }`, not the earlier `{ writer }` — an app
   * had no business naming its own writer (that was the cross-app hole). A
   * manifest still carrying `"oltp": { "writer": "…" }` deserializes to
   * `enabled: undefined` → **disabled**, and `ctx.oltp` then reports the
   * capability as missing. Switch it to `{ "enabled": true }`.
   */
  oltp?: OxyAppEnabledCapability;
  /**
   * Capability for `ctx.airhouse` — append the app's own FACTS (what happened,
   * never edited) to its schema in the workspace's Airhouse (fail-closed: omit →
   * every `ctx.airhouse` call rejected). A pure GATE like `oltp`: the schema is
   * derived from the app's slug (`store-ops` → `app_store_ops`), never named
   * here. Writes run as the app, whoever invoked the function, so a scheduled
   * run can write. Tables come from `airhouseMigrations`.
   */
  airhouse?: OxyAppEnabledCapability;
  /**
   * Retry policy for **background** runs (a `schedule` fire or a manual job
   * trigger). Omit → a job run is attempted once. Route (HTTP) invocations are
   * request-scoped and never retried. `maxAttempts` counts the first try
   * (`maxAttempts: 3` = up to 2 retries); backoff is exponential (doubling)
   * between `minTimeoutMs` and `maxTimeoutMs`. Maps to the durable queue's
   * retry policy — a transient failure re-runs the whole isolate.
   */
  retries?: OxyAppRetriesManifest;
  /**
   * Example input params for the function — a sample JSON body the admin "Run
   * now" surface prefills so an operator knows what to pass (the function reads
   * it as its `req` body, same as a route invocation). Advisory only; not
   * enforced at runtime.
   */
  inputExample?: unknown;
}

/** `functions.<name>.airwayStep`: the pipeline and resource the step transforms. */
export interface OxyAppAirwayStepManifest {
  /** The `.airway.yml` pipeline name. */
  pipeline: string;
  /** The resource (source table) within it. */
  resource: string;
}

/** `functions.<name>.webhook`: how the platform verifies a third party's POST. */
export interface OxyAppWebhookManifest {
  /**
   * App-secret key holding the signing key(s) — the same `apps/<app-id>/`
   * namespace `ctx.env` reads. The manifest names the secret, never holds it.
   *
   * **Comma-separated for rotation.** Providers that keep two live signing
   * keys (Uber's `BASIC_HMAC` does) sign with either during a rotation; any
   * match passes, so adopting a new key does not drop the events still
   * signed with the old one.
   * @pattern \S
   */
  secretVar: string;
  /**
   * Header carrying the signature, e.g. `x-uber-signature`.
   * @pattern \S
   */
  signatureHeader: string;
  /** How the digest is encoded. Default `hex`. */
  encoding?: "hex" | "base64";
  /**
   * A fixed prefix the provider puts before the digest, stripped before it
   * is decoded: GitHub sends `sha256=<hex>` in `x-hub-signature-256`. Declare
   * it and a delivery without it is refused; leave it out for a provider that
   * sends the bare digest (Uber, Toast, Shopify).
   * @pattern \S
   */
  signaturePrefix?: string;
}

/** `functions.<name>.fetch`: per-function `ctx.fetch` limits. */
export interface OxyAppFetchManifest {
  /**
   * Raise this function's `ctx.fetch` response ceiling above the 10 MiB
   * default, in bytes. Opt-in per function rather than a global setting,
   * because the cap protects a SHARED process: one app needing a large
   * document should not raise the ceiling for every app beside it.
   * @asType integer
   * @minimum 1
   */
  maxResponseBytes?: number;
}

/** `functions.<name>.cache`: result caching for route invocations. */
export interface OxyAppCacheManifest {
  /**
   * How long a route invocation's result is served from cache, in seconds.
   * Only for read-only / idempotent functions.
   * @asType integer
   * @minimum 1
   */
  ttlSeconds?: number;
}

/** `functions.<name>.retries`: the durable queue's retry policy for background runs. */
export interface OxyAppRetriesManifest {
  /**
   * Attempts in total, the first try included.
   * @asType integer
   * @minimum 1
   */
  maxAttempts?: number;
  /**
   * The first backoff, in milliseconds.
   * @asType integer
   * @minimum 1
   */
  minTimeoutMs?: number;
  /**
   * The backoff ceiling, in milliseconds.
   * @asType integer
   * @minimum 1
   */
  maxTimeoutMs?: number;
}

/** `functions.<name>.secrets`. */
export interface OxyAppSecretsCapability {
  /** Allow `ctx.secrets.set` on the app's own namespace. */
  write?: boolean;
}

/** `functions.<name>.email`. */
export interface OxyAppEmailCapability {
  /** Allow `ctx.email.send`. */
  send?: boolean;
}

/** `functions.<name>.notify`. */
export interface OxyAppNotifyCapability {
  /** Each kind the function may send, mapped to whether it ignores mutes. */
  kinds?: Record<string, OxyAppNotifyKindCapability>;
}

/** One declared `notify.kinds` entry. */
export interface OxyAppNotifyKindCapability {
  /** A safety alert: delivered past a person's mutes. */
  critical?: boolean;
}

/** `functions.<name>.storage`. */
export interface OxyAppStorageCapability {
  /** Allow `getDownloadUrl`, `list` and `get`. */
  read?: boolean;
  /** Allow `getUploadUrl` and `put`. */
  write?: boolean;
}

/** `functions.<name>.org`. */
export interface OxyAppOrgCapability {
  /** Allow `ctx.org.people()`. */
  read?: boolean;
}

/** `functions.<name>.platform`. */
export interface OxyAppPlatformCapability {
  /** Allow `ctx.platform.facts()` (internal orgs only). */
  facts?: boolean;
}

/** `functions.<name>.oltp` and `functions.<name>.airhouse`: a pure gate. */
export interface OxyAppEnabledCapability {
  /** Open the capability. The target schema is derived from the app's slug. */
  enabled?: boolean;
}

/** Wire shape of `oxy-app.json` (v2 only). */
export interface OxyAppManifest {
  /**
   * The schema this file is written against, for editor completion and
   * validation — `https://unpkg.com/@oxy-hq/sdk/schema/oxy-app.schema.json`,
   * or the installed copy at `./node_modules/@oxy-hq/sdk/schema/oxy-app.schema.json`.
   * Ignored by the platform.
   */
  $schema?: string;
  /** Must be 2. v1 manifests are no longer supported. */
  schemaVersion: 2;
  /**
   * Optional display name. Informational: the name an app shows is its
   * registry row's, set when the row is created (the admin **New app**
   * dialog, or `oxyc publish --name` on a first publish).
   */
  name?: string;
  /**
   * URL slug. **Required.** The canonical source of truth — the
   * dialog locks the slug field to this value, and
   * `OXY_APP_BASE_PATH=/customer-apps/<org>/<slug>/` baked into the
   * build must match.
   *
   * 1–63 lowercase letters, digits and single hyphens, with no leading,
   * trailing or double hyphen and no underscore (`SLUG_PATTERN` in
   * `manifest-rules.ts`, the server's `is_valid_slug`): it becomes the app's
   * served path, a repo path segment and its OLTP schema name.
   */
  slug: string;
  /**
   * One-line purpose, shown on the launcher card under the name.
   */
  description?: string;
  /**
   * Shell-rail icon — a path relative to the bundle root (`icon.svg`), served
   * through the app's own public URL. A small square mark; the rail falls back
   * to a name-initial tile when absent. The Vite plugin copies a file named
   * here into the build output when it sits beside the manifest.
   */
  icon?: string;
  /**
   * Card art for the launcher — a path relative to the bundle root
   * (`card.svg`), served through the app's own public URL. Copied into the
   * build output like `icon`.
   */
  art?: string;
  /**
   * Status line shown on the launcher card (`"23 stores · live"`). A plain
   * display string, static for now.
   */
  status?: string;
  /**
   * Optional org slug. Prefills the dialog's org picker; operator
   * can still override. Carries no security weight — the actual
   * access check is on the linked row.
   *
   * It is also what lets `pnpm dev` and `oxyc publish` find the app without
   * `--org` / `OXY_ORG`: `create-oxy-app --org <slug>` writes it.
   */
  orgSlug?: string;
  /**
   * Optional project (workspace) uuid the bundle expects to read
   * from. Used by `useQuery` to construct the
   * `/api/projects/:id/query` URL.
   */
  projectId?: string;
  /**
   * How `oxyc publish` builds the bundle. Each defaults when absent or blank:
   * `install` to `pnpm install`, `command` to `pnpm build`, `outDir` to `out`
   * — the directory the Vite plugin builds into. The plugin reads `outDir`
   * too, so one setting here moves both.
   */
  build?: OxyAppBuildManifest;
  /**
   * Per-environment targets for `oxyc --env <name>`: the deployment URL a
   * named environment publishes to, when it is not one of the built-in
   * `dev` / `staging` / `production`. Read by `oxyc` only.
   */
  environments?: Record<string, OxyAppEnvironmentManifest>;
  /**
   * Optional map of Oxy Functions (server-side handlers) shipped in the
   * bundle's `functions/` dir, keyed by function name — `^[a-z][a-z0-9-]{0,63}$`
   * (`FUNCTION_NAME_PATTERN`, the server's `is_valid_function_name`), because a
   * key becomes `functions/<name>.js`. Omit for a pure static bundle.
   */
  functions?: Record<string, OxyAppFunctionManifest>;
  /**
   * Schema migrations for the app's OLTP store (`ctx.oltp`) that ship WITH this
   * bundle and run on promote.
   *
   * `dir` is a directory inside the built bundle holding numbered `.sql` files.
   * The platform runs them in lexical order, **once each, ever**, inside a
   * transaction, as the app's own writer role, and records each one.
   *
   * What changes for the author: you no longer write defensive
   * `IF NOT EXISTS` / idempotent upserts, because re-running is a no-op by
   * construction rather than by your care. And you **may not edit, rename or
   * copy a migration that has already run** — all three fail the promote by
   * name, and the fix is always a new file.
   *
   * The `.sql` files are ordinary bundle files, fetchable over the app's own
   * host: put no secrets in them.
   */
  migrations?: OxyAppMigrationsManifest;
  /**
   * Tables for the app's FACTS, in its Airhouse schema `app_<writer>`: a
   * directory of numbered `.sql` files run once each, at promote, with the same
   * ledger rules as `migrations` (never edit, rename or copy one that ran).
   *
   * Every object must be named `app_<writer>.<name>`. Airhouse is DuckLake, so a
   * file declaring a PRIMARY KEY, UNIQUE, an index or a foreign key is refused at
   * publish — a table carrying one fails and leaves the writer inert.
   */
  airhouseMigrations?: OxyAppMigrationsManifest;
  /**
   * What the app's non-production environments (staging) write instead of
   * production. Read by the platform at publish and on every staging call.
   *
   * `destinations` maps a database a function writes in production to the one
   * staging writes instead. In staging, `ctx.warehouse.insert` / `exec` /
   * `upsert` and `ctx.tx` naming `clickhouse` run against
   * `clickhouse_staging`; reads (`ctx.warehouse.query`) stay on production.
   * The mapped database must pass the same checks production's does: listed in
   * the function's `destinations` and, unless it is Airhouse, named in its
   * `customerWarehouseWrites` with a reason. A staging write to a database with
   * no mapping is **held** — not performed, and listed in the invocation's
   * `app.staging.held` audit row.
   *
   * Publish refuses a mapping onto the same database, onto the workspace's own
   * Airhouse (`airhouse_managed` is production's tenant), DuckDB to DuckDB, a
   * chain (a target that is itself a key here), or a target whose database
   * resolves to the same host and user as any production database mapped here —
   * a heuristic, not proof; the credential behind the staging entry is yours to
   * keep separate, with no write grant on production. The platform repeats the
   * check on every staging write, refusing it when a secret does not resolve,
   * and holds a statement that names production's database, or writes through
   * another database than the mapped one's.
   *
   * `ctx.airhouse` needs no mapping: staging's appends land in the app's
   * sibling schema, `app_<writer>__staging`, whose tables come from the same
   * `airhouseMigrations`.
   *
   * ```jsonc
   * "nonProduction": { "destinations": { "clickhouse": "clickhouse_staging" } }
   * ```
   */
  nonProduction?: OxyAppNonProductionManifest;
  /**
   * Optional Ask Oxygen binding (agent ref + composer chips). The
   * platform's registered copy is authoritative (surfaced by
   * shell-context); this local copy is the dev-time fallback so the
   * shell's Ask dock works before the app is registered.
   */
  ask?: OxyAppAskManifest;
  /**
   * The secrets this app expects, keyed by env-var name — the app's
   * `.env.example`, declared rather than written in a README.
   *
   * These are the keys your functions read as `ctx.env.KEY`. Declaring one puts
   * it in the app's Secrets surface (staff console → app → Secrets, and the
   * workspace's own settings) as a row to fill in, so a fresh deploy says what
   * is still missing instead of failing at the first invocation. Every function's
   * `webhook.secretVar` is folded in automatically — no need to repeat it here.
   *
   * **Names only, never values.** A manifest ships inside the bundle and is
   * fetchable over the app's own host; putting a secret in one publishes it.
   * Values are set out-of-band on the Secrets surface, or by
   * `ctx.secrets.set` from a function holding the `secrets.write` capability.
   *
   * App-level rather than per-function, because a secret is app-scoped by
   * construction: two functions sharing `STRIPE_API_KEY` read the same value,
   * so it can only be described once.
   *
   * ```jsonc
   * "env": {
   *   "STRIPE_API_KEY":    { "required": true, "description": "Restricted key, Dashboard → Developers" },
   *   "SLACK_WEBHOOK_URL": { "description": "Optional ops channel" },
   *   "POKEHOUSE_API_KEY": { "shared": true }  // staging reads production's value
   * }
   * ```
   *
   * Read by the platform at **publish time** (like {@link OxyAppStorageManifest}),
   * so the block is documented here but not round-tripped through the dev-time
   * manifest fetch.
   */
  env?: Record<string, OxyAppEnvDeclaration>;
  /**
   * Optional app-level storage policy. Distinct from the per-function
   * `storage: { read, write }` capability: those gate what one function may
   * call, while this governs the app's whole asset silo, which every function
   * shares.
   */
  storage?: OxyAppStorageManifest;
  /**
   * Browser-runtime performance opt-outs. Both features below are **on by
   * default** — an app that says nothing gets them — so this block exists only
   * to turn one off.
   *
   * Read by the platform at **publish time** (like {@link OxyAppStorageManifest})
   * rather than by this loader, so the field is documented here but not
   * round-tripped through the dev-time manifest fetch.
   */
  performance?: OxyAppPerformanceManifest;
  /**
   * Opt out of the platform's automatic, zero-config usage instrumentation:
   * SPA pageviews, Core Web Vitals, engagement time, and uncaught-error counts,
   * posted to `<base>/__oxy/beacon` by the runtime Oxy injects into every served
   * page. `false` silences the **client** runtime only — the server still
   * records one view row per HTML navigation (that floor is not opt-out-able),
   * so the Activity tab never goes dark, it just loses the in-page detail.
   *
   * Distinct from `useTrackEvent` (your own named events): those are additive and
   * always on. This governs only the events the platform sends on your behalf.
   *
   * Honored at publish time (see {@link performance} for why it is not
   * round-tripped here). Default: `true`.
   */
  analytics?: boolean;
  /**
   * The notification catalogue: how each kind the app's functions send
   * (`functions.<name>.notify.kinds`) is labelled, grouped and ranked — the tree
   * a phone groups notifications by, and the one people mute in. Each kind sits
   * in a category path of up to 3 segments (`operations/food-safety`; none →
   * `general`), has a `level` (`passive` | `active` | `time_sensitive` |
   * `critical`) and up to 3 `actions`.
   *
   * Which kinds a function may send is still its own `notify.kinds`; a kind it
   * sends that is missing here gets the defaults (category `general`, the kind
   * humanized, `critical` when marked so). Publish refuses a malformed or
   * unknown category, a bad level, more than 3 actions, an action naming a
   * function the app does not declare, and an `oxy.` kind; `oxyc validate` and
   * the Vite plugin say so first.
   *
   * Read by the platform at **publish time**, so it is documented here but not
   * round-tripped through the dev-time manifest fetch.
   */
  notifications?: OxyAppNotificationsManifest;
  /**
   * Home widgets: live cards on the Oxygen home (behind the apps-first home),
   * keyed by widget id (1–32 lowercase letters, digits, `_` or `-`). Each names
   * a `function` that returns a small JSON view and up to 3 `actions` its
   * buttons may run, each another declared function. The home draws the view
   * itself; no app code runs on the home. At most 3 widgets per app.
   *
   * ```json
   * "widgets": {
   *   "close": {
   *     "title": "Month-end close",
   *     "function": "close-widget",
   *     "size": "wide",
   *     "actions": { "post-ready": { "function": "post-ready-jes", "confirm": true } }
   *   }
   * }
   * ```
   *
   * Production only: widgets read and act on the production build. Publish
   * refuses a bad id, a missing or oversize title, a function the app does not
   * declare, more than 3 widgets or actions, and an unknown key; `oxyc validate`
   * and the Vite plugin say so first. Read by the platform at **publish time**
   * and from the live build, so it is documented here but not round-tripped
   * through the dev-time manifest fetch.
   */
  widgets?: Record<string, OxyAppWidget>;
}

/** The `build` block of `oxy-app.json`: what `oxyc publish` runs. */
export interface OxyAppBuildManifest {
  /** The install step. Default `pnpm install`. */
  install?: string;
  /** The build step, run with `OXY_APP_BASE_PATH` set. Default `pnpm build`. */
  command?: string;
  /** The directory the build writes, relative to the app. Default `out`. */
  outDir?: string;
}

/** One entry of the `environments` block: where a named `--env` publishes. */
export interface OxyAppEnvironmentManifest {
  /** The deployment URL, e.g. `https://acme.oxygen-hq.com`. */
  target?: string;
}

/** The `migrations` / `airhouseMigrations` block: a directory of numbered `.sql` files. */
export interface OxyAppMigrationsManifest {
  /**
   * Directory inside the built bundle, relative to its root.
   * @minLength 1
   */
  dir: string;
}

/** The `ask` block of `oxy-app.json`. */
export interface OxyAppAskManifest {
  /** The agent the Ask dock routes to, as a workspace ref. */
  agent?: string;
  /** Composer chips. */
  suggestedQuestions?: string[];
}

/** The `nonProduction` block of `oxy-app.json`. */
export interface OxyAppNonProductionManifest {
  /** Production database → the database staging writes instead. */
  destinations?: Record<string, string>;
}

/** One declared secret — an entry in the `env` block of `oxy-app.json`. */
export interface OxyAppEnvDeclaration {
  /**
   * Flag the key as **Missing** (rather than merely absent) while nothing is
   * stored for it, and count it in the app's missing-secrets badge.
   *
   * Advisory, not a gate: a publish is never blocked on an unset key, because
   * the first publish is exactly when nobody could have set one yet.
   *
   * Default: `false` — a declaration is documentation first.
   */
  required?: boolean;
  /**
   * Shown beside the key on the Secrets surface. Say what it is and where to
   * get one — this is the text that saves someone a Slack message.
   */
  description?: string;
  /**
   * Outside production (staging), fall back to production's value for this
   * key while the environment holds none of its own. Each environment reads
   * its own secrets (`apps/<id>/<env>/<KEY>`, set on the Secrets surface with
   * the staging option); an unshared key with no staging value reads as
   * **unset**, so staging never holds a production credential nobody chose to
   * give it.
   *
   * For read-only keys only (an API key staging may use as-is). A publish is
   * refused when a shared key is written by a function (`ctx.secrets.set`) or
   * is a `webhook.secretVar` — those must be set per environment — and
   * `ctx.secrets.set` in staging refuses a key the run read through this
   * fallback.
   *
   * Default: `false`.
   */
  shared?: boolean;
}

/** Browser-runtime performance opt-outs — the `performance` block in `oxy-app.json`. */
export interface OxyAppPerformanceManifest {
  /**
   * Opt out of the platform service worker Oxy registers at `<base>/__oxy/sw.js`.
   * It precaches your build's entry assets and serves content-hashed files
   * cache-first, so a repeat load of a published app is near-instant.
   *
   * Set `false` only if your app ships its own service worker (two workers
   * cannot both control the same scope) or genuinely must never be cached.
   * There is nothing to configure to opt *in* — a normal build is precached
   * automatically, and a bundle that inlines everything into one HTML file
   * simply has nothing to precache, which is fine. Default: `true`.
   */
  serviceWorker?: boolean;
  /**
   * How a tab left open across a publish takes the new build. Oxy's injected
   * runtime always notices a publish (within ~5 minutes) and announces it with
   * a cancelable `oxy:update-available` event; this decides what happens next.
   *
   * - `true` — reload at a safe moment (a navigation, two idle minutes with
   *   nothing focused, or coming back after 10+ minutes away), never while the
   *   app holds `useBusyWhile` / `holdUpdates`.
   * - `false` — never reload automatically; read `useAppUpdate()` and offer it.
   * - `"prompt"` — the shell shows "Update ready"; if nobody taps it within 24
   *   hours the runtime reloads at the next safe moment, so an unattended
   *   kiosk still converges.
   *
   * Omitted: the platform default, which an operator sets. A broken lazy chunk
   * after a publish is recovered with a reload in every mode but the
   * platform's `off`.
   */
  autoUpdate?: boolean | "prompt";
}

/** How long assets under a given prefix are kept. */
export interface OxyAppRetentionRule {
  /**
   * Key prefix inside your silo, as you write it — `"tmp/"`, `"generated/"`.
   * The `customer-app-storage/<app_id>/` part is implicit.
   */
  prefix: string;
  /**
   * One of the five supported classes. `null` (or omitted) pins the prefix to
   * "keep forever", which is how you protect it from a broader sibling rule.
   *
   * The set is closed on purpose — each class is one bucket-wide S3 lifecycle
   * rule, so an arbitrary duration can't be honoured. An unrecognized value
   * drops this rule with a warning: a broader sibling rule then applies to the
   * prefix, else it simply doesn't expire. `validateOxyAppManifest` says so as
   * a warning, never a refusal.
   */
  expireAfter?: "1d" | "7d" | "30d" | "90d" | "365d" | null;
}

/** App-level `storage` block in `oxy-app.json`. */
export interface OxyAppStorageManifest {
  /**
   * Retention rules for the asset silo. **Longest matching prefix wins**; a key
   * matching no rule is kept forever.
   *
   * Expiry is enforced by S3 lifecycle rules on an object tag, so it is
   * approximate (evaluated daily, not on the hour) and applies from the time an
   * object was written. Editing a rule does not retag assets already stored —
   * new writes pick up the new class.
   *
   * ```jsonc
   * "storage": {
   *   "retention": [
   *     { "prefix": "tmp/",       "expireAfter": "1d"  },
   *     { "prefix": "generated/", "expireAfter": "90d" },
   *     { "prefix": "uploads/",   "expireAfter": null  }  // keep forever
   *   ]
   * }
   * ```
   */
  retention?: OxyAppRetentionRule[];
}
