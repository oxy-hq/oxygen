/**
 * The host's words, gates, op names and rules, as `@oxy-hq/sdk/testing`
 * enforces them on an app's unit tests.
 *
 * ONE SOURCE OF TRUTH, AND IT IS NOT THIS FILE. Every string below is quoted
 * from the host's source — `host.rs`, `host/destinations.rs`,
 * `host/airhouse_ops.rs`, `upsert_support.rs`, `tx.rs`, `runtime.rs` under
 * `crates/app/src/server/api/custom_apps_functions/`, and the connector's
 * `transaction.rs` / `postgres_tx/convert.rs` — never paraphrased.
 * `{placeholder}`s stand for what the host formats in (`{database}`,
 * `{dialect}`, …) and use the host's own names.
 *
 * HELD TO THE HOST BY A RUST TEST. `sdk_testing_drift.rs`, beside `host.rs`,
 * `include_str!`s this file and fails when a `refusal:` here is no longer a
 * sentence its `source:` file contains, when `HOST_OPS` differs from the
 * host's closed list, when `GATES` stops naming exactly the fields of
 * `FunctionCapabilities`, when `ABSENT_GLOBALS` differs from `oxyc`'s lint
 * list, or when `FETCH_RULES` differs from `is_safe_outbound`. One direction
 * only: this file is checked against the host, the host is never checked
 * against this file (`internal-docs/sdk-testing-context.md` §7, answer 9).
 *
 * THE RUST TEST READS TEXT, NOT TYPESCRIPT: after stripping comments it takes
 * the block from `export const HOST_OPS` to `] as const;`, every object whose
 * key line ends `: {` and carries `source:` and `refusal:` lines, the
 * `hostField:` / `ops:` lines of `GATES`, the `name:` / `member:` lines of
 * `ABSENT_GLOBALS`, and every quoted string in `FETCH_RULES`. Keep those keys
 * spelled as they are, each `refusal:` a single string literal, and none of
 * them behind a comment that is not a full-line one.
 *
 * NO `contractVersion` (§7, answer 10): a reworded refusal reaches an app on
 * its next SDK bump, which is the right moment to see it; a version field
 * with no consumer is a mechanism nobody reads.
 */

/**
 * Every name the host pages an op under — `HOST_OPS` in `host_call_attrs.rs`,
 * "pinned by unit tests to `host_op_name` and `runtime::host_call_span`". The
 * `HostOp` type is this list, so `"warehouse.inserts"` does not compile.
 */
export const HOST_OPS = [
  "query",
  "query_stream",
  "fetch",
  "semantic.query",
  "airway.run",
  "warehouse.insert",
  "warehouse.exec",
  "warehouse.upsert",
  "warehouse.query",
  "tx.begin",
  "tx.begin_oltp",
  "tx.query",
  "tx.exec",
  "tx.commit",
  "tx.rollback",
  "oltp.query",
  "oltp.exec",
  "airhouse.query",
  "airhouse.exec",
  "airhouse.append",
  "storage.getUploadUrl",
  "storage.getDownloadUrl",
  "storage.put",
  "storage.get",
  "storage.head",
  "storage.list",
  "storage.delete",
  "storage.copy",
  "secrets.set",
  "email.send",
  "org.people",
  "org.places",
  "org.assignments"
] as const;

/** One of the host's closed list of op names. */
export type HostOp = (typeof HOST_OPS)[number];

/**
 * The surface `reply_json` prefixes a refusal with (`runtime.rs`): a thrown
 * `HostError`'s message is `"<surface>: <host message>"`. Every `tx.*` verb —
 * `begin_oltp` included — answers under `ctx.tx`, because one op carries all
 * six.
 */
export const SURFACES = {
  query: "ctx.query",
  queryStream: "ctx.queryStream",
  fetch: "ctx.fetch",
  warehouse: "ctx.warehouse",
  tx: "ctx.tx",
  oltp: "ctx.oltp",
  airhouse: "ctx.airhouse",
  secretsSet: "ctx.secrets.set",
  emailSend: "ctx.email.send",
  storage: "ctx.storage",
  orgPeople: "ctx.org.people",
  orgPlaces: "ctx.org.places",
  orgAssignments: "ctx.org.assignments",
  semanticQuery: "ctx.semantic.query",
  airwayRun: "ctx.airway.run"
} as const;

/** One refusal the host writes: the file it is quoted from and its template. */
export interface RefusalTemplate {
  readonly source: string;
  readonly refusal: string;
}

/**
 * Every refusal the context throws, quoted from the host. The context renders
 * a template by substituting its `{placeholder}`s and prefixing the surface.
 */
export const REFUSALS = {
  secretsWrite: {
    source: "host.rs",
    // The host method embeds its own surface, and `reply_json` prefixes it
    // again, so a function sees `ctx.secrets.set: ctx.secrets.set: this …`.
    // Reproduced, not corrected: the context throws what production throws.
    refusal:
      "ctx.secrets.set: this function has not declared the `secrets.write` capability (add it to oxy-app.json to permit writes)"
  },
  emailSend: {
    source: "host.rs",
    refusal:
      'EmailCapabilityMissing: this function has not declared the `email.send` capability (add "email": { "send": true } to its oxy-app.json entry)'
  },
  orgPeople: {
    source: "host.rs",
    refusal:
      'OrgCapabilityMissing: this function has not declared the `org.read` capability (add "org": { "read": true } to its oxy-app.json entry)'
  },
  orgMember: {
    source: "host.rs",
    refusal:
      'OrgCapabilityMissing: this function has not declared the `org.read` capability (add "org": { "read": true } to its oxy-app.json entry) — ctx.org.{member}'
  },
  storageWrite: {
    source: "host.rs",
    refusal:
      'StorageCapabilityMissing: this function has not declared the `storage.write` capability (add "storage": { "write": true } to its oxy-app.json entry)'
  },
  storageRead: {
    source: "host.rs",
    refusal:
      'StorageCapabilityMissing: this function has not declared the `storage.read` capability (add "storage": { "read": true } to its oxy-app.json entry)'
  },
  oltp: {
    source: "host.rs",
    refusal:
      'OltpCapabilityMissing: this function has not declared the `oltp` capability (add "oltp": { "enabled": true } to its oxy-app.json entry). The schema it writes is derived from the app\'s own slug — the manifest only enables access.'
  },
  airhouse: {
    source: "host/airhouse_ops.rs",
    refusal:
      'AirhouseCapabilityMissing: this function has not declared the `airhouse` capability (add "airhouse": { "enabled": true } to its oxy-app.json entry). The schema it writes is derived from the app\'s own slug — the manifest only enables access.'
  },
  destinationNotDeclared: {
    source: "host.rs",
    refusal:
      "database '{database}' is not in this function's `destinations` allowlist (declare it in oxy-app.json to permit writes)"
  },
  databaseNotConfigured: {
    source: "host.rs",
    refusal: "database '{database}' is not configured for this project"
  },
  managedOltpWrite: {
    source: "host/destinations.rs",
    refusal:
      "database '{database}' is the org's OLTP store, which this surface reaches as the read-only analyst — write this app's own records with ctx.oltp"
  },
  customerWarehouseWrite: {
    source: "host/destinations.rs",
    refusal:
      'database \'{database}\' is a customer warehouse, and customer warehouses are read-only to apps. Facts this app records belong in Airhouse (ctx.airhouse); records it edits belong in ctx.oltp. If this write has to stay, say why on the function in oxy-app.json: "customerWarehouseWrites": { "{database}": "<reason>" }'
  },
  customerWarehouseTransaction: {
    source: "host/destinations.rs",
    refusal:
      ". A ctx.tx transaction counts as a write even when it only reads — read with ctx.warehouse.query instead"
  },
  upsertUnsupported: {
    source: "upsert_support.rs",
    refusal:
      "warehouse.upsert is not supported on {dialect}: it compiles to `INSERT … ON CONFLICT … DO UPDATE`, which only Postgres and DuckDB parse. Use warehouse.insert, or warehouse.exec with this warehouse's own upsert statement."
  },
  transactionOpen: {
    source: "host.rs",
    refusal: "could not open a transaction on '{database}': {e}"
  },
  transactionUnsupported: {
    source: "transaction.rs",
    refusal:
      "{backend} does not support multi-statement transactions — ctx.tx() requires a Postgres-backed database (`type: postgres`). Use ctx.warehouse.{insert,exec,upsert} for single-statement writes."
  },
  transactionsOpen: {
    source: "tx.rs",
    refusal:
      "this invocation already has {MAX_OPEN} transactions open. Commit or roll one back before opening another."
  },
  transactionFinished: {
    source: "runtime.rs",
    refusal:
      "{label}: this transaction is already finished — {what} was called after the callback returned"
  },
  transactionFnNotFunction: {
    source: "runtime.rs",
    refusal: "{signature}: fn must be a function"
  },
  btoaBytes: {
    source: "runtime.rs",
    refusal: "btoa: expected a string. For bytes use bytesToBase64() from @oxy-hq/sdk"
  },
  atobPadding: {
    source: "runtime.rs",
    refusal: "atob: '=' may only appear as trailing padding"
  },
  atobLength: {
    source: "runtime.rs",
    refusal: "atob: invalid base64 length"
  },
  fetchInvalidUrl: {
    source: "host.rs",
    refusal: "invalid url: {e}"
  },
  fetchBlocked: {
    source: "host.rs",
    refusal: "fetch to '{url}' blocked by SSRF allowlist"
  },
  fetchInvalidHeaderName: {
    source: "host.rs",
    refusal: "InvalidFetchHeader: `{k}` is not a header name"
  },
  fetchInvalidHeaderValue: {
    source: "host.rs",
    refusal: "InvalidFetchHeader: the value of `{k}` is not a header value"
  },
  fetchTooLarge: {
    source: "host.rs",
    refusal: "response too large ({len} bytes > {max_bytes} cap)"
  },
  // The third state of `WriterCapability` (`host.rs`): the capability is on and
  // the slug still cannot name a schema. Gating on the boolean alone gave a
  // green test for an app whose `ctx.oltp` is closed in production, and the
  // gate-marker scan cannot catch it — this wording shares nothing with the
  // "has not declared the" family.
  slugCannotBackASchema: {
    source: "host.rs",
    refusal:
      "the capability is enabled, but this app's slug '{slug}' cannot back an {store} schema: " +
      "to do so a slug must start with a letter, be at most {max} characters, and use only " +
      "lowercase letters, digits and hyphens (a `_` is refused — it would collide with the " +
      "hyphenated form). A leading digit is a legal app slug but not a legal schema name. " +
      "Rename the app to one that qualifies."
  },
  oltpParamsNotArray: {
    source: "host.rs",
    refusal:
      "`params` must be an array of values. Pass positional arguments for $1, $2, … — e.g. [name, partySize]."
  },
  noDefaultDatabase: {
    source: "host.rs",
    refusal: "this project has no databases configured"
  },
  oltpUnsupportedColumn: {
    source: "postgres_tx/convert.rs",
    refusal:
      "result column `{name}` has Postgres type `{ty}`, which cannot be returned directly. Cast it to text in the SELECT list and parse it in your function — cast the whole expression, not the output name (`avg(qty)::text`, `amount::text`). Note `avg`/`sum` over any numeric type, and a bare decimal literal, are all `numeric`. (Supported as-is: bool, int2/4/8, float4/8, text/varchar/char/name, json/jsonb, uuid, timestamptz/timestamp, date, time.)"
  }
} as const satisfies Record<string, RefusalTemplate>;

/** One capability gate: the `FunctionCapabilities` field and the ops it refuses. */
export interface Gate {
  /** The `FunctionCapabilities` field in `host.rs`. */
  readonly hostField: string;
  /** The capability as the manifest and the refusal name it: `storage.write`. */
  readonly manifest: string;
  /** The host ops refused while the manifest lacks it. */
  readonly ops: readonly HostOp[];
}

/**
 * Every gate in `FunctionCapabilities`, in the struct's order. `storage_read`
 * and `storage_write` list `check_storage_capability`'s arms (`copy` needs
 * both); `customer_warehouse_writes` is decided per database by
 * `destination_write_policy`, after the `destinations` allowlist.
 * `storage_retention` and `fetch_max_bytes` are fields of the struct and
 * deliberately absent: they shape an allowed call, they never refuse one.
 */
export const GATES = [
  {
    hostField: "secrets_write",
    manifest: "secrets.write",
    ops: ["secrets.set"]
  },
  {
    hostField: "email_send",
    manifest: "email.send",
    ops: ["email.send"]
  },
  {
    hostField: "org_read",
    manifest: "org.read",
    ops: ["org.people", "org.places", "org.assignments"]
  },
  {
    hostField: "storage_read",
    manifest: "storage.read",
    ops: ["storage.getDownloadUrl", "storage.get", "storage.head", "storage.list", "storage.copy"]
  },
  {
    hostField: "storage_write",
    manifest: "storage.write",
    ops: ["storage.getUploadUrl", "storage.put", "storage.delete", "storage.copy"]
  },
  {
    hostField: "oltp",
    manifest: "oltp.enabled",
    ops: ["oltp.query", "oltp.exec", "tx.begin_oltp"]
  },
  {
    hostField: "airhouse",
    manifest: "airhouse.enabled",
    ops: ["airhouse.query", "airhouse.exec", "airhouse.append"]
  },
  {
    hostField: "customer_warehouse_writes",
    manifest: "customerWarehouseWrites",
    ops: ["warehouse.insert", "warehouse.exec", "warehouse.upsert", "tx.begin"]
  }
] as const satisfies readonly Gate[];

/**
 * The globals the isolate does not define, as `oxyc`'s `isolate-global` lint
 * names them (`ABSENT_GLOBALS` in `sdk/cli/src/publish/function-lint.ts`).
 * `member` narrows an entry to `<name>.<member>`; an empty `member` is the
 * whole of `process.*`. The isolate's bootstrap defines `Response`, `btoa`,
 * `atob`, `console` and `__buildCtx` and nothing else (`runtime.rs`), so it has
 * no `crypto` global at all; the lint singles out `subtle` because that is what
 * an author reaches for, and `run()` removes the whole global as production
 * does.
 */
export const ABSENT_GLOBALS = [
  { name: "Buffer" },
  { name: "TextEncoder" },
  { name: "TextDecoder" },
  { name: "Blob" },
  { name: "File" },
  { name: "FormData" },
  { name: "crypto", member: "subtle" },
  { name: "process", member: "" }
] as const;

/**
 * `is_safe_outbound` in `host.rs`, the first-layer check every `ctx.fetch`
 * URL meets before anything is sent: the scheme, the host names refused by
 * name, and the suffixes an internal name ends with. The IP rules (loopback,
 * unspecified, private, link-local, broadcast, documentation, v4-mapped and
 * embedded v6) are code, in `fetch-rules.ts`. What no unit test can reproduce
 * is the second layer — a public name that RESOLVES to a private address is
 * caught at connect time by `PublicOnlyDnsResolver` — so the context does not
 * pretend to.
 */
export const FETCH_RULES = {
  scheme: "https",
  literalHosts: ["localhost", "ip6-localhost", "ip6-loopback"],
  internalSuffixes: [".internal", ".local", ".localdomain", ".svc", ".cluster.local"]
} as const;

/** `FETCH_MAX_BYTES` in `host.rs`: the default `ctx.fetch` response ceiling. */
export const FETCH_MAX_BYTES = 10 * 1024 * 1024;

/** `MAX_OPEN` in `tx.rs`: transactions one invocation may hold open at once. */
export const MAX_OPEN_TRANSACTIONS = 4;

/**
 * How each dialect names itself in a refusal — `SqlDialect::as_str` in the
 * connector crate, which `upsert_support::check` and `transaction::unsupported`
 * format in. Only Postgres, DuckDB and SQLite parse `ON CONFLICT`
 * (`parses_on_conflict`), and only the Postgres connector opens a transaction.
 */
export const DIALECTS = {
  clickhouse: { name: "ClickHouse", parsesOnConflict: false, opensTransactions: false },
  postgres: { name: "PostgreSQL", parsesOnConflict: true, opensTransactions: true },
  duckdb: { name: "DuckDB", parsesOnConflict: true, opensTransactions: false }
} as const;

export type Dialect = keyof typeof DIALECTS;
