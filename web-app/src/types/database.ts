interface SemanticEntity {
  name: string;
  description: string;
  sample: string[];
}

interface SemanticDimension {
  name: string;
  description?: string;
  synonyms?: string[];
  sample?: string[];
  type?: string;
  is_partition_key?: boolean;
}

interface SemanticMeasure {
  name: string;
  sql: string;
}

interface SemanticModels {
  table: string;
  database: string;
  description?: string;
  entities?: SemanticEntity[];
  dimensions?: SemanticDimension[];
  measures?: SemanticMeasure[];
  database_name?: string;
}

export interface DatabaseInfo {
  name: string;
  /** SQL dialect for query execution (`"duckdb"`, `"postgres"`, …). */
  dialect: string;
  /** Raw config type from config.yml (`"airhouse_managed"`, `"duckdb"`, …). Use for icons/labels. */
  db_type: string;
  /**
   * `null` when the instance that answered could not look — the semantic sync
   * directory lives in the working copy, which a stateless replica does not
   * have. `{}` means it looked and found none. Only the second may be rendered
   * as "No specific datasets configured"; the first is not ours to claim.
   */
  datasets: Record<string, Record<string, SemanticModels>> | null;
  synced: boolean;
}

interface ColumnInfo {
  name: string;
  data_type: string;
}

export interface TableInfo {
  name: string;
  columns: ColumnInfo[];
}

export interface DatabaseSchema {
  tables: TableInfo[];
}

export interface DatabaseSyncResponse {
  success: boolean;
  message: string;
  sync_time_secs?: number;
}

// Database Configuration Types
interface PostgresConfig {
  host?: string;
  port?: string;
  user?: string;
  password?: string;
  password_var?: string;
  database?: string;
}

interface RedshiftConfig {
  host?: string;
  port?: string;
  user?: string;
  password?: string;
  password_var?: string;
  database?: string;
}

interface MysqlConfig {
  host?: string;
  port?: string;
  user?: string;
  password?: string;
  password_var?: string;
  database?: string;
}

interface ClickHouseConfig {
  host?: string;
  user?: string;
  password?: string;
  password_var?: string;
  database?: string;
}

interface BigQueryConfig {
  /** The key's contents. The server writes them to a key file — local mode only. */
  key?: string;
  /** The secret holding the key's JSON — what a cloud deployment sends instead of `key`. */
  key_json_var?: string;
  dataset?: string;
  dry_run_limit?: number;
}

interface DuckDBConfig {
  file_search_path?: string;
}

interface SnowflakeConfig {
  account?: string;
  username?: string;
  password?: string;
  password_var?: string;
  warehouse?: string;
  database?: string;
  schema?: string;
  role?: string;
  /** A key file on the server's disk — local mode only. */
  private_key_path?: string;
  /** The secret holding the PEM private key — what a cloud deployment sends instead. */
  private_key_var?: string;
  auth_mode?: "password" | "browser" | "private_key";
}

/** `type: airhouse_managed` — the workspace's own Airhouse tenant; nothing to configure. */
type AirhouseManagedConfig = Record<string, never>;

export type DatabaseConfigType =
  | "postgres"
  | "redshift"
  | "mysql"
  | "clickhouse"
  | "bigquery"
  | "duckdb"
  | "snowflake"
  | "airhouse_managed";

type DatabaseConfigValue =
  | PostgresConfig
  | RedshiftConfig
  | MysqlConfig
  | ClickHouseConfig
  | BigQueryConfig
  | DuckDBConfig
  | SnowflakeConfig
  | AirhouseManagedConfig;

export interface WarehouseConfig {
  type: DatabaseConfigType;
  name?: string;
  config: DatabaseConfigValue;
}

export interface WarehousesFormData {
  warehouses: WarehouseConfig[];
}

export interface CreateDatabaseConfigResponse {
  success: boolean;
  message: string;
  databases_added: string[];
}

export interface TestDatabaseConnectionRequest {
  warehouse: WarehouseConfig;
}

export interface TestDatabaseConnectionResponse {
  success: boolean;
  message: string;
  connection_time_ms?: number;
  error_details?: string;
}

export type ConnectionTestEvent =
  | {
      type: "progress";
      message: string;
    }
  | {
      type: "browser_auth_required";
      sso_url: string;
      message: string;
      timeout_secs?: number;
    }
  | {
      type: "complete";
      result: TestDatabaseConnectionResponse;
    };

// Schema Inspection Types (lightweight discovery used during onboarding —
// returns just schema/table names + column counts so the user can pick tables
// before the heavyweight column sync runs).
interface DiscoveredTable {
  name: string;
  column_count: number;
}

interface DiscoveredSchema {
  schema: string;
  tables: DiscoveredTable[];
}

interface InspectionResult {
  schemas: DiscoveredSchema[];
  schema_count: number;
  table_count: number;
  elapsed_ms: number;
}

export type InspectEvent =
  | { type: "progress"; message: string }
  | { type: "complete"; result: InspectionResult }
  | { type: "error"; message: string };

// Schema-first discovery: the onboarding picker first asks for schemas + total
// table counts (one cheap query per warehouse), then lazily fetches tables for
// each schema when the user expands it.
interface SchemaSummary {
  schema: string;
  table_count: number;
}

export interface SchemaListResult {
  schemas: SchemaSummary[];
  elapsed_ms: number;
}

export interface SchemaTablesResult {
  schema: string;
  tables: DiscoveredTable[];
  elapsed_ms: number;
}
