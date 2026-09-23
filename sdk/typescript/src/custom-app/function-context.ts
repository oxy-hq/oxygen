// Author-facing types for **Oxy Functions** — the server-side TypeScript
// handlers bundled inside a custom app (`functions/<name>.ts`, declared in
// `oxy-app.json`). Each function is `export default async (req, ctx) => Response`.
//
// Until now functions were written untyped: the `ctx` object is assembled
// entirely by the Rust host (`__buildCtx` in
// `crates/app/src/server/api/custom_apps_functions/runtime.rs`) and never had
// a TypeScript counterpart. `OxyFunctionContext` below is that counterpart —
// it mirrors the host-provided `ctx.*` members one-for-one so function authors
// get autocomplete + type-checking. Import it as:
//
// ```ts
// import type { OxyFunctionContext, OxyFunctionRequest } from "@oxy-hq/sdk";
//
// export default async function notify(req: OxyFunctionRequest, ctx: OxyFunctionContext) {
//   const html = render(Welcome, { name });
//   const { messageId } = await ctx.email.send({ to, subject, html });
//   return Response.json({ ok: true, messageId });
// }
// ```

// ── Request ──────────────────────────────────────────────────────────────────

/**
 * The request passed as the first argument to a function's default export.
 *
 * The host hands the isolate the raw request body as a string (see
 * `req_json` in `runtime.rs`); parse it yourself, e.g.
 * `JSON.parse(req.body || "{}")`. This is intentionally *not* a full Web
 * `Request` — there is no `.json()` / headers object in v1.
 */
export interface OxyFunctionRequest {
  /** Raw request body as received (JSON string for a JSON POST). */
  body: string;
}

// ── ctx sub-APIs (mirror `__buildCtx`) ────────────────────────────────────────

/** A single row from a `ctx.query` / `ctx.queryStream` result. */
export type OxyFunctionRow = Record<string, unknown>;

/** One org team the caller belongs to, as reported by {@link OxyFunctionUser.teams}. */
export interface OxyOrgTeam {
  id: string;
  name: string;
}

/**
 * Who — or what — invoked this function.
 *
 * `"system"` means **no caller to attribute this to** — not necessarily "no
 * human caused it". A schedule tick, an Airway transform step, and an operator's
 * manual *Run now* all take this path: they run under the org owner's `id` (the
 * invocation record needs a real user FK) with every caller field absent. So on
 * a manual run a person really did click, and there is still no way to reach
 * them; the platform does not carry the triggering operator through the job
 * queue.
 *
 * Any branch that emails "the person who clicked" or renders a personal view
 * must check this rather than sniff the synthetic `email` — and must have a
 * sensible answer for the case where there is nobody to send to.
 */
export type OxyIdentityKind = "user" | "system";

/**
 * Identity of the invoking user (route) or the system identity (schedule,
 * Airway step, or a manual job run).
 *
 * Assembled server-side on every invocation from the authenticated session —
 * **nothing on it is client-supplied**, which is the entire reason to read
 * identity here instead of from the request body. See
 * `internal-docs/custom-apps-user-identity.md` for the full contract, including
 * what the client-side `useShellContext()` can and cannot be trusted for.
 */
export interface OxyFunctionUser {
  /**
   * `users.id`. On a `"system"` invocation this is the org owner's id and not a
   * caller — check {@link kind} before attributing anything to it.
   */
  id: string;
  /**
   * Their email; `schedule+<fn>@system.oxy` when {@link kind} is `"system"`; and
   * **`null` for a frontline worker** — a crew member enrolled by PIN on a shared
   * device has no mailbox, and the platform stores none rather than inventing
   * one. That null is the one field that tells the crew from the office inside
   * a function, because a worker can never hold org membership (see
   * `orgRole`, which is absent for them too). Treat it as `string | null` in
   * app logic; it was typed `string` before the crew existed.
   */
  email: string | null;
  /**
   * The org that owns this app — the tenant boundary for anything the function
   * reads or writes.
   *
   * Servers before 2026-08-21 mistakenly sent this as `org_id`, so `orgId` read
   * `undefined` there; both keys are populated now. If your function filters SQL
   * on it, that is exactly the bug to re-check.
   */
  orgId: string;
  /** Display name. Absent on a `"system"` invocation. User-controlled free text —
   *  fine for a greeting or an audit row, never a key, and escape it before it
   *  reaches HTML or SQL. */
  name?: string;
  /** Avatar URL. Absent when unset or on a `"system"` invocation. */
  picture?: string;
  /**
   * The caller's role **within this app**, derived server-side from app
   * membership (with org-owner / Oxy-staff break-glass). Absent when they hold
   * no membership.
   *
   * This is the value to gate a privileged surface on — it cannot be forged by
   * the client, unlike a query param or a client-side flag:
   *
   * ```ts
   * if (ctx.user.appRole !== "admin") {
   *   return Response.json({ error: "forbidden" }, { status: 403 });
   * }
   * ```
   *
   * Note it is deliberately NOT the org role: an app admin administers one app
   * without holding org-Admin (which also carries billing and member management).
   *
   * A `"system"` invocation runs under the org owner, so this reads `"admin"`
   * there — a schedule carries owner authority by construction. Add a
   * {@link kind} check when a surface must be human-only.
   */
  appRole?: "admin" | "member";
  /**
   * The caller's role in the owning **org**. Absent when they reach the app
   * without an org membership (Oxy staff on break-glass) or on a `"system"`
   * invocation.
   *
   * Informational, not a gate — org standing and app standing are separate
   * rings. Use it to explain ("ask your org admin to connect a warehouse"), to
   * label, or to route; gate on {@link appRole}.
   */
  orgRole?: "owner" | "admin" | "member";
  /**
   * The org teams the caller belongs to, name-sorted, and scoped to this app's
   * org — teams they hold in other orgs are never reported. Empty when they
   * belong to none.
   *
   * Optional because a server older than 2026-08-21 does not send it: use
   * `ctx.user.teams?.some(...)`, never `ctx.user.teams.some(...)`, or the
   * function throws on that server rather than degrading.
   *
   * Useful for *shaping* a view (default the Finance team to the finance tab).
   * Not a permission: a team only grants anything on an app through an app team
   * grant, which is already folded into {@link appRole}. Gating on a team name
   * invents a permission the platform cannot revoke.
   */
  teams?: OxyOrgTeam[];
  /**
   * Whether there is a caller to attribute this invocation to.
   *
   * On a current server this is exact — `ctx.user.kind === "system"` is the
   * check.
   *
   * Optional for the same reason as {@link teams}: a server older than
   * 2026-08-21 does not send it. Note there is no safe *inference* to fall back
   * on, in either direction — `=== "system"` reads `false` for a cron tick, and
   * `!== "user"` reads `true` for a real person. An older server genuinely
   * cannot tell you.
   *
   * So if you must support one, don't infer: a schedule invokes the function
   * with the `input` you configured on it, which is yours to mark.
   *
   * ```ts
   * const body = JSON.parse(req.body || "{}");
   * const isSystem = ctx.user.kind ? ctx.user.kind === "system" : body._trigger === "schedule";
   * ```
   */
  kind?: OxyIdentityKind;
  /**
   * Where the caller may act, derived by the platform from their assignments
   * (`internal-docs/operating-graph.md` §3.3): a system invocation or an app
   * admin everywhere; a holder of an org-wide position everywhere; an
   * assigned person exactly their places; an unassigned org member
   * everywhere and an unassigned frontline worker nowhere. Apply it with
   * `@oxy-hq/sdk/ops` (`requireReach`, `predicate`); tighten it if the app
   * needs to, never widen it. A lookup failure lands on nowhere.
   */
  reach: OxyReach;
}

/** `ctx.user.reach` — see {@link OxyFunctionUser.reach}. */
export interface OxyReach {
  everywhere: boolean;
  /** Why everywhere, when it is; `null` when scoped or nowhere. */
  via: "system" | "app-admin" | "org-wide-position" | "org-member" | null;
  /** The caller's assigned location ids, in id order; empty when none. */
  locations: string[];
}

/** Result of a `ctx.fetch` call. */
export interface OxyFetchResult {
  status: number;
  /** Response body, decoded per the requested {@link OxyFetchInit.encoding}. */
  body: string;
  /** Echoes how `body` was encoded (`"utf8"` unless base64 was requested). */
  encoding?: "utf8" | "base64";
}

/**
 * `init` for `ctx.fetch` — the standard `RequestInit` fields the host honours
 * (`method`, `headers`, `body`) plus how to decode the response.
 */
export type OxyFetchInit = RequestInit & {
  /**
   * How to decode the response body. `"utf8"` (default) is **lossy for
   * binary** — every non-UTF-8 byte becomes U+FFFD, so a fetched PDF/PNG comes
   * back corrupt. Pass `"base64"` for any binary response, e.g. to hand it
   * straight to an email attachment.
   */
  encoding?: "utf8" | "base64";
};

/**
 * `ctx.warehouse.*` — one of the app's configured databases by name.
 *
 * The writes require the database in the function's `destinations` allowlist;
 * `query` does not, because that allowlist is about modifying a project's
 * warehouse, and a `postgres_managed` database resolves the read-only analyst
 * for every caller regardless.
 *
 * **Shape: the customer's warehouse — read it, don't write it.** Writes to a
 * customer warehouse (anything but `airhouse` / `airhouse_managed`) are refused
 * unless the function names the database in `customerWarehouseWrites` with a
 * reason. Facts your app records go to `ctx.airhouse`, records it edits to
 * `ctx.oltp`, files to `ctx.storage`.
 */
export interface OxyWarehouseApi {
  /**
   * Read from a named database.
   *
   * `ctx.query` only ever reaches the project's DEFAULT database, so this is
   * how an app reads its own per-org OLTP store, which sits beside whatever
   * warehouse the project analyses.
   */
  query(database: string, sql: string): Promise<{ rows: OxyFunctionRow[]; truncated: boolean }>;
  insert(database: string, table: string, rows: OxyFunctionRow[]): Promise<unknown>;
  exec(database: string, sql: string): Promise<unknown>;
  /**
   * Insert rows, updating any whose `conflictColumns` already exist.
   *
   * Compiles to `INSERT … ON CONFLICT … DO UPDATE`, which only Postgres and
   * DuckDB parse, and which needs a primary key or unique constraint on
   * `conflictColumns`. On any other warehouse — ClickHouse, Snowflake, BigQuery,
   * MySQL — the call is refused by name before anything is sent; use `exec` with
   * that warehouse's own upsert statement instead. Airhouse (DuckLake) tables
   * carry no such constraint, so there the engine refuses it.
   */
  upsert(
    database: string,
    table: string,
    rows: OxyFunctionRow[],
    conflictColumns: string[]
  ): Promise<unknown>;
}

/**
 * The handle `ctx.tx` and `ctx.oltp.tx` pass to your callback — a pinned
 * connection with an open transaction.
 *
 * Both methods take **bound parameters** (`$1`, `$2`, …). Never build SQL by
 * concatenating request data: `ctx.warehouse.exec` takes a bare string, but a
 * transaction exists for surfaces that accept end-user input, and placeholders
 * are the only thing that makes that safe.
 *
 * The handle is live only for the duration of the callback. Using it after the
 * callback returns throws — it is not a connection you can stash.
 */
export interface OxyTransaction {
  /** Run a row-returning statement (including `INSERT … RETURNING`). */
  query(sql: string, params?: unknown[]): Promise<OxyFunctionRow[]>;
  /** Run a statement for its effect; resolves to the number of rows affected. */
  exec(sql: string, params?: unknown[]): Promise<number>;
}

/**
 * **Shape: records your app edits** — current state that needs constraints or
 * transactions: a booking, a shift assignment, a template, a count. What
 * happened (history that will not change) goes to `ctx.airhouse`; bytes go to
 * `ctx.storage`.
 *
 * `ctx.oltp` — read and WRITE the app's OWN per-org OLTP schema (`app_<writer>`)
 * on the managed Postgres tenant, and nothing else.
 *
 * This is the write half `ctx.warehouse` cannot give an app: for a
 * `postgres_managed` database `ctx.warehouse` resolves the read-only analyst
 * (org-wide read, the org's `raw_*` extracts included), so a write authenticates
 * and then fails `permission denied`. `ctx.oltp` resolves the app's **writer**
 * role instead — DML rights scoped to the one `app_<writer>` schema, so it is
 * narrower on reads (no `raw_*`) and finally writable.
 *
 * Gated by the fail-closed `oltp` manifest capability (`"oltp": { "enabled":
 * true }`) — a pure gate. The target schema is derived from the app's own slug
 * (`oltp-bookings` → `app_oltp_bookings`), never named in the manifest, so a
 * manifest cannot point `ctx.oltp` at another app's schema. The store must be
 * provisioned first (ask whoever operates the org). No database name is passed —
 * the app's own store is implicit.
 *
 * Both methods take **bound parameters** (`$1`, `$2`, …). Never build SQL by
 * concatenating request data — a booking form is exactly the surface that takes
 * end-user input, and placeholders are the only thing that makes it safe. Each
 * call auto-commits; a failed statement rolls back.
 *
 * **Cost:** each call opens its own connection to the tenant (a TCP + TLS
 * handshake, and a wake-up if the compute was idle) and its own transaction, so
 * a per-row loop pays that per row. Prefer one statement over many — a
 * multi-row `INSERT`, an `INSERT … SELECT`, or `INSERT … RETURNING` to avoid a
 * follow-up read — or run several inside one `ctx.oltp.tx`, which holds a single
 * connection. Reach for `ctx.oltp` a handful of times per request, not in a hot
 * loop.
 *
 * ```ts
 * const [row] = await ctx.oltp.query(
 *   "INSERT INTO bookings (name, party_size) VALUES ($1, $2) RETURNING id",
 *   [name, partySize],
 * );
 * ```
 */
export interface OxyOltpApi {
  /** Run a row-returning statement (including `INSERT … RETURNING`). */
  query(sql: string, params?: unknown[]): Promise<OxyFunctionRow[]>;
  /** Run a statement for its effect; resolves to the number of rows affected. */
  exec(sql: string, params?: unknown[]): Promise<number>;
  /**
   * Run several statements as one transaction on one connection: commits when
   * `fn` resolves, rolls back when it throws, and rethrows your error. The same
   * handle and rules as `ctx.tx` — let a failed statement's error propagate —
   * without naming a database: the app's own store is implicit.
   *
   * ```ts
   * await ctx.oltp.tx(async (tx) => {
   *   await tx.exec("UPDATE shifts SET status = 'closed' WHERE id = $1", [shiftId]);
   *   await tx.exec("INSERT INTO shift_notes (shift_id, body) VALUES ($1, $2)", [shiftId, note]);
   * });
   * ```
   */
  tx<T>(fn: (tx: OxyTransaction) => Promise<T> | T): Promise<T>;
}

/**
 * **Shape: facts — what happened, which will not change**: an order, a completed
 * checklist, a delivery, a reading. `ctx.airhouse` appends them to your app's own
 * schema in the workspace's Airhouse, where the analytics agent, semantic views
 * and other apps read them as history. Records your app edits in place belong in
 * `ctx.oltp`; files in `ctx.storage`.
 *
 * Gated by the fail-closed `airhouse` manifest capability (`"airhouse": {
 * "enabled": true }`), a pure gate: the schema is `app_<writer>`, derived from
 * the app's slug (`store-ops` → `app_store_ops`) and exposed as `schema`. Writes
 * run **as the app**, whoever invoked the function — a schedule, a webhook and a
 * click write the same way.
 *
 * Every statement is checked before it is sent. Reads may name any schema;
 * writes must target `<schema>.<table>`; `exec` runs no DDL — declare tables in
 * `airhouseMigrations` files, which run once at publish. One statement per call.
 *
 * Airhouse is DuckLake: **no primary keys, UNIQUE, indexes or foreign keys**, and
 * no bound parameters. Give every fact the id its source assigned and a
 * `recorded_at`, append a correction as a new fact instead of updating, and keep
 * one row per id when reading — a retried function can append twice:
 *
 * ```ts
 * await ctx.airhouse.append("checklist_completions", [
 *   { completion_id: id, store_guid: store, recorded_at: new Date().toISOString() },
 * ]);
 *
 * const { rows } = await ctx.airhouse.query(`
 *   SELECT * FROM ${ctx.airhouse.schema}.checklist_completions
 *   QUALIFY row_number() OVER (PARTITION BY completion_id ORDER BY recorded_at DESC) = 1
 * `);
 * ```
 */
export interface OxyAirhouseApi {
  /** `app_<writer>` when the function declares the capability, else `null`. */
  readonly schema: string | null;
  /** Read, from any schema. Capped like `ctx.warehouse.query`; `truncated` says the cap cut rows. */
  query(sql: string): Promise<{ rows: OxyFunctionRow[]; truncated: boolean }>;
  /**
   * Append rows to `<schema>.<table>`. Pass the bare table name; every row must
   * carry the first row's columns. Values are sent as literals, safely quoted.
   * Resolves to the number of rows sent.
   */
  append(table: string, rows: OxyFunctionRow[]): Promise<number>;
  /**
   * One write statement against your own schema: a `DELETE` for retention or
   * erasure, an `INSERT … SELECT` deriving facts from other facts. Nothing is
   * bound, so never interpolate request data here — that is what `append` is for.
   */
  exec(sql: string): Promise<void>;
}

/**
 * `ctx.secrets` — write app-scoped secrets (gated by the `secrets.write` capability).
 *
 * **Credentials only — not state.** A secret is for a value you authenticate
 * with, like a rotated token. A cursor, a counter or a JSON blob that changes
 * between runs is a record: keep it in `ctx.oltp`.
 */
export interface OxySecretsApi {
  set(key: string, value: string): Promise<void>;
}

/**
 * `ctx.crypto` — HMAC signing and verification, and a constant-time compare.
 * **Synchronous**: pure CPU inside the isolate, so these skip the host-call
 * channel and there is nothing to `await`. Mirrors `crypto` in `__buildCtx`
 * (`runtime.rs`); the three members here are the three the host binds.
 *
 * `key` and `data` are read as UTF-8 — every webhook scheme in the wild signs a
 * UTF-8 base string with a UTF-8 secret (GitHub the body, Slack `v0:ts:body`,
 * Stripe `ts.body`) — so there is deliberately no per-argument encoding knob.
 *
 * Who controls an input decides what its absence does. An unset or empty `key`,
 * or an unknown `algorithm` / `encoding`, is the author's mistake and **throws**.
 * A `signature` that is absent or will not decode is the caller's, and returns
 * **`false`** — throwing would turn a forged request into a 500. Both sides of
 * `timingSafeEqual` are symmetric, so an absent or empty side is `false` and
 * never a throw: with the secret unset, every request is rejected.
 */
export interface OxyCryptoApi {
  /**
   * The digest of `data` under `key`, as a string in `encoding`. For signing an
   * **outbound** request; `verifyHmac` is the inverse direction.
   */
  hmac(input: OxyHmacInput): string;
  /**
   * Whether `signature` is the digest of `data` under `key`, compared in
   * constant time. Strip the provider's prefix first (`sha256=`, `v0=`) and
   * pass the bare digest — prefix formats are per-provider. A header the caller
   * omitted can be passed as-is: absent is `false`, not a throw.
   */
  verifyHmac(input: OxyVerifyHmacInput): boolean;
  /**
   * Constant-time equality for a plain shared secret where there is no HMAC.
   * Use it, not `===`, for any secret comparison: `===` short-circuits at the
   * first differing byte and leaks the secret one byte at a time to anyone who
   * can time the endpoint. `false` when either side is absent or empty.
   */
  timingSafeEqual(a: string | null | undefined, b: string | null | undefined): boolean;
}

/** The inputs `ctx.crypto.hmac` and `ctx.crypto.verifyHmac` share. */
export interface OxyHmacInput {
  /** `"sha256"` (default) or `"sha512"`. Anything else throws. */
  algorithm?: "sha256" | "sha512";
  /**
   * The secret, from configuration (`ctx.env.…`), never from the request.
   * Required and non-empty: an absent one throws rather than signing with `""`
   * or the literal `"undefined"` — keys an attacker guesses as easily as you do.
   */
  key: string;
  /** The base string to sign — for a webhook, the body or `v0:{ts}:{body}`. */
  data: string;
  /** How the digest is written (`hmac`) or read (`verifyHmac`): `"hex"` (default) or `"base64"`. */
  encoding?: "hex" | "base64";
}

/** `ctx.crypto.verifyHmac`'s input: {@link OxyHmacInput} plus the signature to check. */
export interface OxyVerifyHmacInput extends OxyHmacInput {
  /**
   * The bare digest the caller sent, provider prefix (`sha256=`, `v0=`) already
   * stripped. Attacker-controlled, so a header the caller omitted may be passed
   * as-is: absent, or not decodable in `encoding`, is `false`, never a throw.
   */
  signature: string | null | undefined;
}

/** `ctx.semantic` — airlayer-compiled semantic queries (inherits the pre-agg fast path). */
export interface OxySemanticApi {
  /**
   * Run a semantic query. `scope: "reach"` pins it server-side to the
   * caller's `ctx.user.reach`: one `in` filter per view the query names whose
   * primary entity is bound to the org's locations registry, over the keys the
   * caller's places carry in that system. A caller who reaches everywhere is
   * left alone; a query naming no bound view is refused rather than answered
   * whole. `internal-docs/operating-graph.md` §3.6.
   */
  query(spec: Record<string, unknown> & { scope?: "reach" }): Promise<unknown>;
}

/** `ctx.airway` — seed/await an Airway ELT pipeline run. */
export interface OxyAirwayApi {
  run(pipelineRef: string, variables?: Record<string, unknown> | null): Promise<{ runId: string }>;
}

// ── Email ─────────────────────────────────────────────────────────────────────

/**
 * Input to `ctx.email.send`. Platform-injected: the sender mailbox (`from`) is
 * platform-controlled and **not** an accepted field — passing it is a typed
 * error. Provide `html` and/or `text` as the body (render a template to HTML
 * with `render` from `@oxy-hq/sdk/email`).
 */
export interface EmailSendInput {
  /** Recipient address(es). Required. */
  to: string | string[];
  /** CC address(es). */
  cc?: string | string[];
  /** BCC address(es). */
  bcc?: string | string[];
  /** Reply-To address — the only sender-identity field an author may set. */
  replyTo?: string;
  /** Subject line. Required. */
  subject: string;
  /** HTML body. Provide at least one of `html` / `text`. */
  html?: string;
  /** Plain-text body. Provide at least one of `html` / `text`. */
  text?: string;
  /**
   * Optional idempotency key (≤256 chars). Accepted and validated in v1 but a
   * no-op until the persisted idempotency table lands — adopt it now so
   * background (retried) sends become exactly-once once it does.
   */
  idempotencyKey?: string;
  /**
   * Files to attach. Max 20 per send, and **10 MiB decoded in total** — SES
   * caps a whole message near 40 MB, so for anything larger store the file with
   * {@link OxyStorageApi} and email a presigned link instead of inlining it.
   *
   * `content` is base64 by default; for generated text set
   * `encoding: "utf8"` and attach the string as-is.
   */
  attachments?: EmailAttachment[];
}

/** One attachment on {@link EmailSendInput}. */
export interface EmailAttachment {
  /** Filename shown to the recipient. Required; path separators are stripped. */
  filename: string;
  /**
   * File contents, interpreted per {@link EmailAttachment.encoding} — base64 by
   * default, which is the only way binary crosses the isolate boundary.
   */
  content: string;
  /**
   * How `content` is encoded. Defaults to `"base64"`.
   *
   * Use `"utf8"` to attach text the function just generated (CSV, JSON, HTML)
   * — it needs no encoder and is byte-exact for non-ASCII. `btoa` is the wrong
   * tool there: it encodes U+0080..U+00FF as *Latin1*, so accented text comes
   * out as mojibake rather than as an error. For binary, take base64 straight
   * from the source — `ctx.storage.get(key, { encoding: "base64" })` or
   * `ctx.fetch(url, { encoding: "base64" })` — or {@link bytesToBase64} for a
   * `Uint8Array` you built yourself.
   */
  encoding?: "base64" | "utf8";
  /** MIME type; defaults to `application/octet-stream`. */
  contentType?: string;
  /** Render inline (e.g. an image referenced as `cid:<contentId>`) instead of as a download. */
  inline?: boolean;
  /** Content-ID for an inline part, referenced from the HTML body as `cid:<contentId>`. */
  contentId?: string;
}

/** Result of a successful `ctx.email.send`. */
export interface EmailSendResult {
  /** Provider (SES) message id of the sent message. */
  messageId: string;
}

/** `ctx.email` — send email (gated by the `email.send` capability). */
export interface OxyEmailApi {
  send(input: EmailSendInput): Promise<EmailSendResult>;
}

// ── Storage ───────────────────────────────────────────────────────────────────

/** Input to `ctx.storage.getUploadUrl`. */
export interface StorageUploadUrlInput {
  /**
   * Destination path inside the app's silo, e.g. `"uploads/q1-report.pdf"`.
   * Segments are sanitized server-side and cannot escape the silo. Omit to use
   * `filename`, which is placed under `uploads/`.
   */
  pathname?: string;
  /** Shorthand for `pathname: "uploads/<filename>"`. */
  filename?: string;
  /** MIME type; bound into the presigned PUT signature. Inferred when omitted. */
  contentType?: string;
  /**
   * Exact byte length of the upload, bound into the signature — S3 rejects a
   * body of any other size. Capped by the server's upload ceiling (100 MiB by
   * default).
   */
  contentLength: number;
  /** Presign lifetime in seconds (default 900; max 604800 — SigV4's own limit). */
  expiresInSeconds?: number;
}

/** A minted presigned upload. */
export interface StorageUploadUrl {
  /** Presigned PUT — the browser uploads the file bytes directly to this URL. */
  url: string;
  /**
   * The stored key. Record it (e.g. on a row in your warehouse) — it is how you
   * fetch, list or link to the asset later. A random suffix is added so two
   * people uploading `report.pdf` don't collide.
   */
  key: string;
  /** ISO-8601 expiry of the presigned URL. */
  expiresAt: string;
  /**
   * Retention tag for this key, present only when your app declares a matching
   * `storage.retention` rule in `oxy-app.json` (e.g. `"oxy-ttl=30d"`).
   *
   * **When present, the upload MUST send it as the `x-amz-tagging` header** — it
   * is bound into the signature, so omitting it fails the PUT with a signature
   * mismatch rather than storing an untagged object:
   *
   * ```ts
   * const { url, tagging } = await ctx.storage.getUploadUrl({ ... });
   * await fetch(url, {
   *   method: "PUT",
   *   body: file,
   *   headers: {
   *     "Content-Type": file.type,
   *     ...(tagging ? { "x-amz-tagging": tagging } : {}),
   *   },
   * });
   * ```
   *
   * Signing it is deliberate: a browser that could drop the header could opt any
   * upload out of the app's own retention policy.
   */
  tagging?: string;
}

/** A minted presigned download. */
export interface StorageDownloadUrl {
  url: string;
  expiresAt: string;
}

/** One asset in the app's silo. */
export interface StorageObject {
  key: string;
  size: number;
  contentType?: string | null;
  /** ISO-8601. */
  lastModified?: string | null;
}

/** One page of {@link OxyStorageApi.list}. */
export interface StorageListPage {
  objects: StorageObject[];
  /** Pass back as `cursor` to fetch the next page; `null` when complete. */
  cursor: string | null;
  hasMore: boolean;
}

/** Options for {@link OxyStorageApi.put}. */
export interface StoragePutOptions {
  /** MIME type. Inferred from the pathname's extension when omitted. */
  contentType?: string;
  /**
   * How `body` is encoded. `"base64"` is what makes **binary** generated assets
   * (PDF, PNG, Parquet) possible — a UTF-8 string would corrupt them.
   */
  encoding?: "utf8" | "base64";
  /** Append a short random component before the extension to avoid collisions. */
  addRandomSuffix?: boolean;
  /**
   * Replace an existing asset at this path. Defaults to `false` — writing over
   * an asset by accident is worse than an error, so this is opt-in.
   */
  allowOverwrite?: boolean;
  /** `Cache-Control: max-age=<seconds>` stored on the object. */
  cacheControlMaxAge?: number;
}

/** Result of a `put` (and of `copy`). */
export interface StoragePutResult {
  key: string;
  size: number;
  contentType: string;
}

/**
 * `ctx.storage` — this app's **asset store**, covering both kinds of file an app
 * produces, in one silo (`customer-app-storage/<app_id>/`):
 *
 * - **Uploaded** — a human picks a file; `getUploadUrl` mints a presigned PUT and
 *   the browser uploads **straight to S3**, so uploads aren't bounded by the
 *   request-body limit and the bytes never pass through your function.
 * - **Generated** — your function produces the file (a rendered PDF, a CSV
 *   export, a chart PNG) and writes it with `put`, using
 *   `{ encoding: "base64" }` for binary.
 *
 * Gated by the fail-closed `storage.read` / `storage.write` capabilities in
 * `oxy-app.json`. Every asset is private; reads are always presigned and
 * time-boxed. Keys are confined to your app — another app's key is rejected.
 *
 * ```ts
 * // Uploaded: mint a URL, browser PUTs to it, then record `key`.
 * const { url, key } = await ctx.storage.getUploadUrl({
 *   filename: "q1-report.pdf", contentType: "application/pdf", contentLength: size,
 * });
 *
 * // Generated: write a CSV your function just built.
 * const { key } = await ctx.storage.put("generated/jan.csv", csv);
 *
 * // Either way: email a link that outlives the request.
 * const { url: link } = await ctx.storage.getDownloadUrl(key, {
 *   expiresInSeconds: 604800, download: true,
 * });
 * ```
 */
export interface OxyStorageApi {
  /** Mint a presigned PUT for a browser upload (requires `storage.write`). */
  getUploadUrl(input: StorageUploadUrlInput): Promise<StorageUploadUrl>;
  /**
   * Mint a presigned GET (requires `storage.read`). `download: true` forces a
   * save-as via `Content-Disposition`, which is what an emailed link wants.
   */
  getDownloadUrl(
    key: string,
    opts?: { expiresInSeconds?: number; download?: boolean }
  ): Promise<StorageDownloadUrl>;
  /**
   * Write a generated asset (requires `storage.write`). Capped at 6 MiB — for
   * anything larger, mint a presigned upload URL and stream to it.
   */
  put(pathname: string, body: string, opts?: StoragePutOptions): Promise<StoragePutResult>;
  /** Read an asset back; `null` when absent (requires `storage.read`). */
  get(
    key: string,
    opts?: { encoding?: "utf8" | "base64" }
  ): Promise<{ body: string; contentType: string | null; size: number; encoding: string } | null>;
  /** Metadata without the body; `null` when absent (requires `storage.read`). */
  head(key: string): Promise<StorageObject | null>;
  /**
   * One page of assets (requires `storage.read`). Paginated deliberately — pass
   * the returned `cursor` back to walk a large silo without loading it all.
   */
  list(opts?: { prefix?: string; limit?: number; cursor?: string }): Promise<StorageListPage>;
  /**
   * Delete one or many assets (requires `storage.write`). Idempotent — deleting
   * an absent key is a no-op success. `deleted` is the number of keys **accepted**
   * for deletion (an absent key counts too), not a count of keys that existed.
   */
  delete(keyOrKeys: string | string[]): Promise<{ deleted: number }>;
  /**
   * Server-side copy within the app's silo (requires `storage.read` **and**
   * `storage.write`: it reads the source and writes the destination).
   */
  copy(
    fromKey: string,
    toPathname: string,
    opts?: { allowOverwrite?: boolean }
  ): Promise<StoragePutResult>;
}

// ── ctx ───────────────────────────────────────────────────────────────────────

/** One of the org's locations, as `ctx.org.places()` returns it. */
export interface OxyOrgPlace {
  id: string;
  org_id: string;
  name: string;
  /** The tenant's word for this level — `region`, `store` — or `null`. */
  kind: string | null;
  /** The place this one sits inside, or `null` for a root. */
  parent_id: string | null;
  status: "pre_launch" | "launching" | "open" | "archived" | "terminated";
  /** IANA zone; what "due by close" means here. */
  timezone: string;
  /** The tenant's own id, if any. */
  external_id: string | null;
  /** `system` → id: what Toast, a camera console, payroll call this place. */
  external_ids: Record<string, string>;
  created_at: string;
  updated_at: string;
}

/** One assignment — a person holding a position at a place (or org-wide). */
export interface OxyOrgAssignment {
  id: string;
  user_id: string;
  user_name: string;
  user_kind: "member" | "frontline";
  role_id: string;
  role_name: string;
  /** `location` — held at one place; `franchisor` — held across the org. */
  role_scope: "location" | "franchisor";
  /** `null` for an org-wide position. */
  location_id: string | null;
  location_name: string | null;
  /** Who they report to at that place, if recorded. */
  supervisor_id: string | null;
  supervisor_name: string | null;
  created_at: string;
}

/**
 * The data-plane context passed as the second argument to a function's default
 * export. Mirrors the host-assembled `ctx` (`__buildCtx` in `runtime.rs`);
 * every member is a host-provided async function bridged to a Rust backend,
 * except `crypto`, which is synchronous (pure CPU inside the isolate).
 */
export interface OxyFunctionContext {
  /** Invoking user (route) or system identity (schedule/airway). */
  user: OxyFunctionUser;
  /**
   * The org's people directory. Requires `"org": { "read": true }` in this
   * function's manifest entry — without it the call is rejected before any
   * query reaches the database.
   *
   * For naming a person: an assignee, a roster entry, who submitted something.
   * Returns a display name and a role, and deliberately **no email, no phone,
   * no location**.
   *
   * Who is in it: **people who can reach this app**. Org members, plus frontline
   * workers holding a grant on this app — `kind` tells them apart, so a caller
   * that must not name a worker can refuse on the field rather than by
   * convention. A worker's `role` is `null`: that vocabulary is org membership's
   * and a worker has none.
   *
   * REQUIRED, like every sibling here — `oltp`, `secrets`, `email`, `storage`,
   * `airway` are all gated and all declared required. The binding is
   * unconditional: `__buildCtx` is a static string that attaches `org` whatever
   * the manifest says, and the refusal lives in the op, not in the binding. An
   * optional member would therefore be a lie in the other direction, and under
   * `strict` it makes `ctx.org.people()` — the spelling in every doc here and
   * the only one the host binds — fail with "possibly undefined".
   */
  org: {
    people(): Promise<{
      people: Array<{
        id: string;
        name: string;
        /** The org role, or `null` for a frontline worker. */
        role: string | null;
        kind: "member" | "frontline";
      }>;
      total: number;
    }>;
    /**
     * The org's places — every location, with its hierarchy (`parent_id`,
     * the tenant-named `kind`), lifecycle `status`, `timezone`, and what each
     * integration calls it (`external_ids`, e.g. `{ toast: "…" }`). The whole
     * registry, not a reach-scoped slice: an app that shows "Clovis" needs
     * the row before it knows whether the caller reaches it. Same `org.read`
     * capability as `people()`.
     */
    places(): Promise<{ places: OxyOrgPlace[]; total: number }>;
    /**
     * Who holds which position where — the roster, read. Scoped like
     * `people()`: the assignments of people who can reach this app. Same
     * `org.read` capability. Rosters are edited in Settings, not from a
     * function.
     */
    assignments(): Promise<{ assignments: OxyOrgAssignment[]; total: number }>;
  };
  /** Read-only view of the app's configured secrets (project-scoped). */
  env: Record<string, string>;
  /** Structured per-invocation logging (captured + surfaced with the response). */
  log(...args: unknown[]): void;
  /**
   * HMAC sign / verify and a constant-time compare. Synchronous — no `await`.
   * See {@link OxyCryptoApi}.
   */
  crypto: OxyCryptoApi;
  /**
   * Read-only SQL (`SELECT` / `WITH` only) against the app's default database,
   * capped at the function row limit. Resolves to `{ rows, truncated }` — the
   * shape the host sends (`host.rs` `query`), the same as `ctx.warehouse.query`;
   * `truncated` says the cap cut rows. Destructure it:
   * `const { rows } = await ctx.query(sql)`.
   */
  query(sql: string): Promise<{ rows: OxyFunctionRow[]; truncated: boolean }>;
  /** Read-only SQL with a higher row cap, yielded to the caller in batches. */
  queryStream(
    sql: string,
    opts?: { batchSize?: number }
  ): AsyncGenerator<OxyFunctionRow[], void, unknown>;
  /**
   * SSRF-allowlisted outbound HTTP with a response-size cap. Pass
   * `{ encoding: "base64" }` for a binary response — the default UTF-8 decode
   * corrupts it.
   */
  fetch(url: string, init?: OxyFetchInit): Promise<OxyFetchResult>;
  warehouse: OxyWarehouseApi;
  /**
   * Run several statements atomically on one connection: commits when your
   * callback resolves, rolls back when it throws, and rethrows your error
   * either way. Resolves to whatever the callback returns.
   *
   * `database` must be in this function's manifest `destinations` — a
   * transaction is a write, and the same fail-closed allowlist applies. Postgres
   * only; other backends reject `ctx.tx` rather than faking it. A customer
   * warehouse is refused unless named in `customerWarehouseWrites`; for the app's
   * own OLTP store use `ctx.oltp.tx`, which needs no destination. A transaction
   * counts as a write even when your callback only reads — `begin` cannot know —
   * so for reads alone use `ctx.warehouse.query`.
   *
   * **Do not catch a failed statement and return normally.** A statement the
   * server rejects aborts the whole transaction, and `COMMIT` on an aborted
   * transaction does not fail — Postgres applies nothing and reports success —
   * so `ctx.tx` refuses to commit and throws instead, naming the statement that
   * poisoned it. Let the error propagate.
   *
   * ```ts
   * const orderId = await ctx.tx("appdb", async (tx) => {
   *   const [{ id }] = await tx.query(
   *     "INSERT INTO orders (table_no) VALUES ($1) RETURNING id",
   *     [tableNo],
   *   );
   *   for (const it of items) {
   *     await tx.exec(
   *       "INSERT INTO order_items (order_id, sku, qty) VALUES ($1, $2, $3)",
   *       [id, it.sku, it.qty],
   *     );
   *   }
   *   return id;
   * });
   * ```
   */
  tx<T>(database: string, fn: (tx: OxyTransaction) => Promise<T> | T): Promise<T>;
  /**
   * **Records your app edits.** Read/write the app's OWN per-org OLTP schema
   * (derived from its slug), with `ctx.oltp.tx` for several statements at once.
   * Gated by the fail-closed `oltp` manifest capability (`{ enabled: true }`).
   * See {@link OxyOltpApi}.
   */
  oltp: OxyOltpApi;
  /**
   * **Facts: what happened.** Append-only history in the app's own Airhouse
   * schema, written as the app. Gated by the fail-closed `airhouse` manifest
   * capability (`{ enabled: true }`). See {@link OxyAirhouseApi}.
   */
  airhouse: OxyAirhouseApi;
  /** Credentials the app rotates — not a state store. See {@link OxySecretsApi}. */
  secrets: OxySecretsApi;
  semantic: OxySemanticApi;
  airway: OxyAirwayApi;
  email: OxyEmailApi;
  /** **Files: bytes.** The app's private storage silo. See {@link OxyStorageApi}. */
  storage: OxyStorageApi;
}

/** Signature of a function's default export: `export default async (req, ctx) => Response`. */
export type OxyFunctionHandler = (
  req: OxyFunctionRequest,
  ctx: OxyFunctionContext
) => Promise<Response> | Response;
