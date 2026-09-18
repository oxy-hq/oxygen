/**
 * Pure `.airway.yml` scaffold builder.
 *
 * The "New Pipeline" wizard collects: name, description, a source, and
 * a destination *database* (referenced by name from the project's
 * `config.yml`). The open-ended source connector config is left as a
 * commented placeholder the user fills in the YAML editor; the
 * destination carries no credentials — the backend resolves the named
 * database into a connection string at run time (incl. per-user
 * `airhouse_managed` minting).
 *
 * A source *option* is what the user picks; `airwayKind` is what gets
 * written as the YAML `kind:` and must be one of the dispatch arms in
 * `agentic_airway::source_factory`. Vendor sources (e.g. Toast) are
 * `rest_api` under the hood with a vendor-flavoured config template.
 */

/** Connector kinds wired in `agentic_airway::source_factory`. */
type AirwaySourceKind =
  | "rest_api"
  | "filesystem"
  | "sql_database"
  | "clickhouse"
  | "postgres_cdc"
  | "toast"
  | "quickbooks"
  | "sp_api";

export interface SourceOption {
  /** Selection id — may differ from the airway kind (e.g. "toast"). */
  id: string;
  label: string;
  description: string;
  /** The `kind:` written to YAML (an airway source_factory arm). */
  airwayKind: AirwaySourceKind;
}

export const SOURCE_OPTIONS: SourceOption[] = [
  {
    id: "rest_api",
    label: "REST API",
    description: "Paginated JSON endpoints (most SaaS APIs)",
    airwayKind: "rest_api"
  },
  {
    id: "toast",
    label: "Toast POS",
    description: "Toast restaurant POS (orders, menus, labor)",
    airwayKind: "toast"
  },
  {
    id: "quickbooks",
    label: "QuickBooks Online",
    description: "Accounting + inventory (invoices, bills, P&L)",
    airwayKind: "quickbooks"
  },
  {
    id: "sp_api",
    label: "Amazon Selling Partner",
    description:
      "Seller or Vendor Central reports (FBA inventory, shipments, orders, listings, vendor sales)",
    airwayKind: "sp_api"
  },
  {
    id: "filesystem",
    label: "Filesystem",
    description: "Local or cloud object storage (S3, GCS, Azure)",
    airwayKind: "filesystem"
  },
  {
    id: "sql_database",
    label: "SQL Database",
    description: "Full / query-based extract from a relational DB",
    airwayKind: "sql_database"
  },
  {
    id: "clickhouse",
    label: "ClickHouse",
    description: "Extract over the ClickHouse HTTP interface (JSONEachRow)",
    airwayKind: "clickhouse"
  },
  {
    id: "postgres_cdc",
    label: "Postgres CDC",
    description: "Change-data-capture via logical replication",
    airwayKind: "postgres_cdc"
  }
];

/**
 * config.yml database types airway can write to. The backend resolver
 * maps these to an airway destination kind (postgres -> postgres,
 * airhouse / airhouse_managed -> airhouse).
 */
export const WRITABLE_DESTINATION_DB_TYPES = [
  "postgres",
  "redshift",
  "airhouse",
  "airhouse_managed"
] as const;

/** First day of the PREVIOUS month, `YYYY-MM-DD`.
 *
 * The default start, and the reasoning is a balance between two failure modes
 * that are not symmetric.
 *
 * Reaching too far back is the dangerous one. `plan_pull` emits a single
 * `Window { start, end: now }` — the whole span is ONE report, never chunked —
 * and the cursor only advances on success. So if the first window is large
 * enough that Amazon cannot build the report inside the poll budget (20 polls,
 * roughly 40-75 minutes) or the document cannot be downloaded inside the 300s
 * deadline, the run fails and *every later run retries the identical window*.
 * It never gets smaller on its own. That is a permanent stall, not a slow start.
 *
 * Reaching too recent loses history quietly: this connector only pulls forward,
 * so anything before the first run is absent until someone resets the cursor.
 *
 * A month boundary makes the span self-limiting — between one and two months
 * depending on the day of the month, never more — which is comfortably inside
 * both budgets while still giving the demand model a full prior month.
 *
 * UTC to match the connector, which stamps and compares in UTC throughout.
 */
export function firstOfLastMonth(now: Date = new Date()): string {
  // `Date.UTC` normalises month -1 into the previous year in January.
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - 1, 1))
    .toISOString()
    .slice(0, 10);
}

/** Per-option `config:` block, keyed by `SourceOption.id`. */
const SOURCE_CONFIG: Record<string, string> = {
  rest_api: `    base_url: https://api.example.com
    # Auth, pagination and per-endpoint options follow airway's
    # RestApiConfig. An empty list extracts nothing — add endpoints.
    endpoints: []`,
  // Fallback only — the wizard always supplies real Toast fields and
  // routes through `buildToastConfig`.
  toast: `    client_id: <toast-client-id>
    client_secret_var: TOAST_CLIENT_SECRET
    restaurant_guids:
      - "<restaurant-guid>"`,
  // QuickBooks Online. `client_secret_var` / `refresh_token_var` are
  // secret-manager names resolved at run time; the rotated refresh token
  // is written back to `refresh_token_var` after each refresh. Use
  // base_url for the sandbox.
  quickbooks: `    client_id: <intuit-client-id>
    client_secret_var: QB_CLIENT_SECRET
    refresh_token_var: QB_REFRESH_TOKEN
    realm_id: <company-realm-id>
    # base_url: https://sandbox-quickbooks.api.intuit.com`,
  // Amazon SP-API. `client_secret_var` / `refresh_token_var` are
  // secret-manager names resolved at run time. `marketplace_id` must be a
  // NORTH AMERICA marketplace — the connector pins the NA endpoint, and the
  // factory refuses anything else by name rather than letting it 403 like a
  // bad credential. `default_start` is required: the connector pulls forward
  // only, so it is the entire backfill policy.
  sp_api: `    client_id: <lwa-client-id>
    client_secret_var: SP_API_CLIENT_SECRET
    refresh_token_var: SP_API_REFRESH_TOKEN
    marketplace_id: ATVPDKIKX0DER # US; CA=A2EUQ1WTGCTBG2
    partner_type: seller # or vendor — these are separate Amazon accounts
    default_start: "<YYYY-MM-DD>"`,
  filesystem: `    base_path: /path/to/data # or s3://bucket/prefix, gs://..., az://...
    pattern: "*.jsonl"
    format: jsonl # json | jsonl | csv
    table_name: my_table`,
  sql_database: `    connection_string: postgresql://user:pass@host:5432/db
    backend: postgres # postgres | mysql | sqlite | mssql | oracle | custom
    tables:
      - name: my_table
        write_disposition: append # append | replace | merge`,
  clickhouse: `    host: my-host.clickhouse.cloud
    port: 8443 # HTTP(S) interface port (8123 plaintext / 8443 TLS)
    database: default
    username: default
    # Resolved from the secret manager at run time — the secret value
    # is never written into the .airway.yml.
    password_var: CLICKHOUSE_PASSWORD
    secure: true
    tables:
      - name: my_table
        # cursor_field: created_at # incremental high-watermark column
        write_disposition: append # append | replace | merge`,
  postgres_cdc: `    connection_string: postgresql://user:pass@host:5432/db
    slot_name: oxy_slot
    publication_name: oxy_pub
    tables:
      - my_table
    initial_snapshot: true`
};

/** Toast wizard fields. `clientSecretVar` is the secret-manager name
 *  the executor resolves at run time — the secret itself is never
 *  written into the `.airway.yml`. */
interface ToastScaffold {
  clientId: string;
  clientSecretVar: string;
  restaurantGuids: string[];
  baseUrl?: string;
}

/** QuickBooks Online wizard fields. `clientSecretVar` / `refreshTokenVar`
 *  are secret-manager names resolved at run time — the secret values
 *  themselves are never written into the `.airway.yml`. Intuit rotates the
 *  refresh token on every use; the executor writes the rotated value back
 *  to `refreshTokenVar`. */
interface QuickBooksScaffold {
  clientId: string;
  clientSecretVar: string;
  refreshTokenVar: string;
  realmId: string;
  baseUrl?: string;
}

/** Which Amazon account a set of SP-API credentials belongs to.
 *
 * Not a filter over one roster. Seller Central and Vendor Central are separate
 * accounts with separate authorizations, and a refresh token belongs to one of
 * them for as long as it exists — so the connector publishes only the reports
 * the configured account can reach, and the two need SEPARATE pipelines. */
export type SpApiPartnerType = "seller" | "vendor";

/** The secret names the wizard offers, per account.
 *
 * Distinct per account on purpose, and it is the one default here that
 * prevents a silent wrong answer rather than saving typing. A vendor pipeline
 * offered `SP_API_REFRESH_TOKEN` — the seller pipeline's name — passes
 * validation with a blank value, because a secret under that name already
 * exists and blank means "reuse it". The vendor pipeline would then run with
 * the SELLER's token and be refused on every vendor report, which reads as a
 * credentials problem rather than as the wrong credential. */
export const SP_API_SECRET_DEFAULTS: Record<
  SpApiPartnerType,
  { clientSecret: string; refreshToken: string }
> = {
  seller: {
    clientSecret: "SP_API_CLIENT_SECRET",
    refreshToken: "SP_API_REFRESH_TOKEN"
  },
  vendor: {
    clientSecret: "SP_API_VENDOR_CLIENT_SECRET",
    refreshToken: "SP_API_VENDOR_REFRESH_TOKEN"
  }
};

/** Re-default a secret name when the account changes, without clobbering one
 * the operator typed.
 *
 * Only rewrites a name that still equals the OTHER account's default, so
 * switching seller/vendor back and forth tracks the defaults while any custom
 * name survives untouched. Pure and exported so this is pinned by a test
 * rather than by whichever wizard state happens to reach it.
 */
export function retargetSpApiSecretName(
  current: string,
  from: SpApiPartnerType,
  to: SpApiPartnerType,
  field: "clientSecret" | "refreshToken"
): string {
  return current === SP_API_SECRET_DEFAULTS[from][field]
    ? SP_API_SECRET_DEFAULTS[to][field]
    : current;
}

/** Amazon SP-API wizard fields.
 *
 * `clientSecretVar` / `refreshTokenVar` are secret-manager names resolved at
 * run time — the values themselves never reach the `.airway.yml`. Unlike
 * QuickBooks there is no OAuth flow here: an SP-API refresh token comes from
 * authorizing the app in the account's own console — Seller Central or Vendor
 * Central — so the operator already holds one.
 *
 * `defaultStart` is REQUIRED, not optional, and that is deliberate — see
 * `buildSpApiConfig`. `partnerType` is required for a different reason: the
 * wizard now asks the question, and a scaffold that could omit the answer
 * would be one that sometimes wrote a pipeline nobody had chosen an account
 * for. */
interface SpApiScaffold {
  clientId: string;
  clientSecretVar: string;
  refreshTokenVar: string;
  marketplaceId: string;
  defaultStart: string;
  partnerType: SpApiPartnerType;
}

/** ClickHouse wizard fields. `passwordVar` is the secret-manager name
 *  the executor resolves at run time — the password itself is never
 *  written into the `.airway.yml`. `tables` are the names picked from
 *  the discovery table picker. */
export type WriteDisposition = "append" | "replace" | "merge";

/** One picked ClickHouse table + how it should be loaded on each run. */
export interface ClickHouseScaffoldTable {
  name: string;
  /** `append` (insert), `replace` (full overwrite), `merge` (upsert). */
  writeDisposition: WriteDisposition;
  /** High-water-mark column for incremental `append` (only new rows). */
  cursorField?: string;
  /** Upsert key for `merge` (one or more columns). */
  primaryKey?: string[];
}

interface ClickHouseScaffold {
  host: string;
  port?: number;
  database: string;
  username?: string;
  passwordVar?: string;
  secure?: boolean;
  tables: ClickHouseScaffoldTable[];
}

export interface ScaffoldInput {
  name: string;
  description?: string;
  /** A `SourceOption.id`. */
  sourceId: string;
  /** Required when `sourceId === "toast"`. */
  toast?: ToastScaffold;
  /** Required when `sourceId === "quickbooks"`. */
  quickbooks?: QuickBooksScaffold;
  /** Required when `sourceId === "clickhouse"`. */
  clickhouse?: ClickHouseScaffold;
  /** Required when `sourceId === "sp_api"`. */
  spApi?: SpApiScaffold;
  /** A `config.yml` database name (the resolved destination). */
  destinationDatabase: string;
  /** Logical dataset/schema written into the destination. */
  datasetName: string;
  /** Whether the chosen destination is airhouse-backed. Gates the
   *  ClickHouse `schema_separator` default — it's an airhouse-only knob,
   *  and other destination kinds reject the unknown field at run start. */
  destinationIsAirhouse?: boolean;
}

function buildToastConfig(t: ToastScaffold): string {
  const guids = (t.restaurantGuids.length ? t.restaurantGuids : ["<restaurant-guid>"])
    .map((g) => `      - "${g}"`)
    .join("\n");
  const lines = [
    `    client_id: ${t.clientId}`,
    `    client_secret_var: ${t.clientSecretVar}`,
    "    restaurant_guids:",
    guids
  ];
  if (t.baseUrl?.trim()) lines.push(`    base_url: ${t.baseUrl.trim()}`);
  return lines.join("\n");
}

function buildQuickBooksConfig(q: QuickBooksScaffold): string {
  // realm_id is an all-digits company id (e.g. 9341456860808037). Quote it
  // so YAML keeps it a string — unquoted it parses as an integer and the
  // connector's `realm_id: String` field rejects it ("invalid type: integer").
  const lines = [
    `    client_id: ${q.clientId}`,
    `    client_secret_var: ${q.clientSecretVar}`,
    `    refresh_token_var: ${q.refreshTokenVar}`,
    `    realm_id: "${q.realmId}"`
  ];
  if (q.baseUrl?.trim()) lines.push(`    base_url: ${q.baseUrl.trim()}`);
  return lines.join("\n");
}

function buildSpApiConfig(s: SpApiScaffold): string {
  // `default_start` is always emitted, never omitted-to-default. The
  // connector pulls FORWARD only, so this single value is the entire backfill
  // policy: too recent and the history is simply absent with nothing to signal
  // it, too far back and the first run spends a report job per chunk against a
  // per-account budget that restores about once a minute. `build_sp_api`
  // refuses a config without it rather than guessing, so the wizard must not
  // produce one either.
  //
  // Quoted so YAML keeps `2026-01-01` a string rather than parsing it as a
  // date — the connector's field is a String it parses itself, and an unquoted
  // date arrives as a serde type error naming the struct, not the field.
  //
  // `partner_type` is emitted ALWAYS, including for `seller` where it matches
  // the connector's default. A generated file should say which account it is
  // without the reader having to know what the default was — and writing it
  // only for vendor would make the vendor case look like the special one,
  // when the two are simply different accounts.
  return [
    `    client_id: ${s.clientId}`,
    `    client_secret_var: ${s.clientSecretVar}`,
    `    refresh_token_var: ${s.refreshTokenVar}`,
    `    marketplace_id: ${s.marketplaceId}`,
    `    partner_type: ${s.partnerType}`,
    `    default_start: "${s.defaultStart}"`
  ].join("\n");
}

function buildClickHouseConfig(c: ClickHouseScaffold): string {
  const lines = [`    host: ${c.host}`];
  if (c.port != null) lines.push(`    port: ${c.port}`);
  lines.push(`    database: ${c.database}`);
  if (c.username?.trim()) lines.push(`    username: ${c.username.trim()}`);
  if (c.passwordVar?.trim()) lines.push(`    password_var: ${c.passwordVar.trim()}`);
  if (c.secure != null) lines.push(`    secure: ${c.secure}`);
  lines.push("    tables:");
  if (c.tables.length === 0) {
    lines.push("      - name: <table-name>");
    lines.push("        write_disposition: append # append | replace | merge");
  }
  for (const t of c.tables) {
    lines.push(`      - name: ${t.name}`);
    // append with a cursor = incremental (only rows past the high-water
    // mark); without one, append re-loads everything each run.
    if (t.writeDisposition === "append" && t.cursorField) {
      lines.push(`        cursor_field: ${t.cursorField}`);
    }
    if (t.writeDisposition === "merge" && t.primaryKey?.length) {
      lines.push("        primary_key:");
      for (const k of t.primaryKey) lines.push(`          - ${k}`);
    }
    lines.push(`        write_disposition: ${t.writeDisposition}`);
  }
  return lines.join("\n");
}

/** Build the initial `.airway.yml` body for a freshly-created pipeline. */
export function buildPipelineScaffold(input: ScaffoldInput): string {
  const option = SOURCE_OPTIONS.find((o) => o.id === input.sourceId) ?? SOURCE_OPTIONS[0];
  let configBlock: string;
  if (option.id === "toast" && input.toast) {
    configBlock = buildToastConfig(input.toast);
  } else if (option.id === "quickbooks" && input.quickbooks) {
    configBlock = buildQuickBooksConfig(input.quickbooks);
  } else if (option.id === "clickhouse" && input.clickhouse) {
    configBlock = buildClickHouseConfig(input.clickhouse);
  } else if (option.id === "sp_api" && input.spApi) {
    configBlock = buildSpApiConfig(input.spApi);
  } else {
    configBlock = SOURCE_CONFIG[option.id] ?? SOURCE_CONFIG[option.airwayKind];
  }
  const desc = input.description?.trim();
  const descLine = desc ? `description: ${JSON.stringify(desc)}\n` : "";
  // ClickHouse has no schemas, so tables are commonly flattened as
  // `<schema>___<table>`. Split that back into real destination schemas
  // (e.g. analytics___jobs -> analytics.jobs); names without `___` stay
  // under dataset_name. Remove this line to keep one flat root schema.
  // Airhouse-only knob — other destination kinds reject the field at run
  // start, so only emit it when the destination is airhouse-backed.
  const schemaSepLine =
    option.id === "clickhouse" && input.destinationIsAirhouse ? '\n  schema_separator: "___"' : "";
  return `name: ${input.name}
${descLine}source:
  kind: ${option.airwayKind}
  config:
${configBlock}
destination:
  # A database defined in config.yml. Credentials are resolved at run
  # time (airhouse_managed mints an ephemeral per-user credential).
  database: ${input.destinationDatabase}
  dataset_name: ${input.datasetName}${schemaSepLine}

# Optional: restrict to a subset of the source's resources.
# resources: []

# In-flight extractions when running resources in parallel (1-16).
concurrency: 1

# Streaming (concurrent extract->sink, live per-table progress) is on
# by default for streaming-capable destinations. Uncomment to force
# the bulk path.
# streaming: false
`;
}
