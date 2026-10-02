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

- This is an **infrastructure crate** — shared by analytics and builder domains.
- Config construction (secret resolution, project path resolution) happens in `agentic-pipeline::platform`, NOT here. This crate only knows about `ConnectorConfig` values, never credentials or secret managers.
- `build_connector()` / `build_named_connectors()` are the entry points for creating connector instances from configs.
- **A value written into SQL text goes through `connector.string_literal().quote(value)`** (`string_literal.rs`), never a hand-rolled `replace('\'', "''")`. Doubling the quote is only enough on DuckDB and Postgres: ClickHouse, MySQL, Snowflake, Redshift and Domo also read a backslash, and BigQuery reads `\'` but not `''`. Ask the *connector*, not the dialect — Redshift reports the Postgres dialect. A wrapper connector must forward `string_literal()` from its inner one. Bind parameters where the backend supports it (`SqlTransaction`).
