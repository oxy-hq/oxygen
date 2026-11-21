# agentic-connector

Database connectivity abstraction. Provides a unified `DatabaseConnector` trait over multiple backends.

## Supported Backends

| Feature flag | Connector | Backend |
| ------------- | ----------- | --------- |
| `duckdb` | `DuckDbConnector` | Bundled DuckDB |
| `postgres` | `PostgresConnector` | `tokio-postgres` |
| `airhouse` | `AirhouseConnector` | `tokio-postgres` (pgwire, DuckDB dialect) |
| `mysql` | `MysqlConnector` | `sqlx-mysql` |
| `clickhouse` | `ClickHouseConnector` | HTTP API via `reqwest` |
| `snowflake` | `SnowflakeConnector` | `snowflake-api` |
| `bigquery` | `BigQueryConnector` | `gcp-bigquery-client` |
| `domo` | `DomoConnector` | REST API via `reqwest` |

## Key Types

```rust
pub trait DatabaseConnector: Send + Sync {
    async fn execute_query(&self, sql: &str) -> Result<QueryResult, ConnectorError>;
    async fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError>;
    fn dialect(&self) -> SqlDialect;
}

pub enum ConnectorConfig {
    DuckDb(DuckDbConfig),
    Postgres(PostgresConfig),
    Redshift(PostgresConfig),
    ClickHouse(ClickHouseConfig),
    Snowflake(SnowflakeConfig),
    BigQuery(BigQueryConfig),
    DuckDbRaw(DuckDbRawConfig),
    DuckDbUrl(DuckDbUrlConfig),
}
```

## Rules

- This is an **infrastructure crate** — shared by the analytics and automation domains.
- Config construction (secret resolution, project path resolution) happens in `agentic-pipeline::platform`, NOT here. This crate only knows about `ConnectorConfig` values, never credentials or secret managers.
- `build_connector()` / `build_named_connectors()` are the entry points for creating connector instances from configs.
- **A value written into SQL text goes through `connector.string_literal().quote(value)`** (`string_literal.rs`), never a hand-rolled `replace('\'', "''")`. Doubling the quote is only enough on DuckDB and Postgres: ClickHouse, MySQL, Snowflake, Redshift and Domo also read a backslash, and BigQuery reads `\'` but not `''`. Ask the *connector*, not the dialect — Redshift reports the Postgres dialect. A wrapper connector must forward `string_literal()` from its inner one. Bind parameters where the backend supports it (`SqlTransaction`).
- **A `ConnectorConfig::DuckDb` directory is built once per process, not per run** (`duckdb/dir_cache.rs`): `build_connector` hands each caller a `try_clone()` of a cached database, keyed on the directory, load strategy and every file's size + mtime. Clones only *read* the shared catalog — before anything that both the lexer (`duckdb/read_only.rs`, the fast route) and DuckDB's own parser (`duckdb/parse_check.rs`: every statement prepared on an empty shadow catalog must be a `SELECT`) do not vouch for, the connector moves onto a private copy (`Sessions::turn_for`), so one run's `CREATE TABLE` never reaches another. That build runs on the blocking pool, once per connector, holding no lock other statements wait on; statements queue for their turn asynchronously. Pre-aggregation skips the cache (`DuckDbConfig::reuse_cached_directory: false`, set by `OxyProjectContext::with_own_directory_databases`): every rollup writes first, so a clone only added the shared build to a cold cycle. Objects in the shared database are registered non-`TEMP` because temp objects are invisible to clones. `DuckDbConnector::from_directory` stays uncached.
- **`execute_query_read_only` is the react `query` tool's only path, and must hold with every caller check deleted** (`supports_read_only` says whether a connector has one; the trait default refuses and never falls through to `execute_query`). Postgres/Redshift: `BEGIN READ ONLY` on the one client under its mutex, verified, the SQL only through the extended protocol inside one wrapper statement, always `ROLLBACK` then `pg_advisory_unlock_all()`, `SET LOCAL` statement/lock timeouts, a role that can write server files (superuser, `pg_write_server_files`, `lo_export`) refused, and a client dropped mid-transaction is discarded (`postgres_read_only.rs`). DuckDB: a second, private database over a directory connector's files with external access off, `allowed_paths` = the view files, configuration locked, and a rolled-back transaction; its parse check binds on a shadow locked the same way, since binding opens files and URLs (`duckdb/locked.rs`) — DuckDB's file settings are instance-wide, so never set them on a dir-cache clone or a handed-in connection; `DuckDbConnector::new` therefore has no read-only path. A wrapper connector forwards both methods.
