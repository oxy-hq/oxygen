//! Airhouse integration for Oxy.
//!
//! Airhouse is a managed analytics warehouse that speaks the PostgreSQL wire
//! protocol but executes SQL in the DuckDB dialect. This crate owns everything
//! Oxy needs to talk to Airhouse:
//!
//! - **`connector`** feature — `AirhouseConnector` (pgwire transport, DuckDB
//!   dialect). Implements `agentic_connector::DatabaseConnector`. Pulls in
//!   only the trait crate + `tokio-postgres` so it is safe to enable from
//!   `agentic-pipeline` without breaking that crate's no-platform-deps rule.
//! - **`credentials`** feature — Sea-ORM entity for `airhouse_tenants` plus
//!   the airhouse migrator. Pulled in by `oxy` core to surface
//!   `DatabaseType::AirhouseManaged` config errors and run airhouse
//!   migrations on startup.
//! - **`admin`** feature — implies `credentials`. Adds the admin HTTP client
//!   (`AirhouseAdminClient`), tenant provisioner, SA-backed token broker,
//!   env-driven config loader, and the local-mode seeder. Adds `oxy-auth`
//!   (used by the seeder for the `Identity` type).
//! - **`rest`** feature — Axum handlers for
//!   `/airhouse/me/{connection,credentials,provision,tokens/:username}`.
//!   Requires `admin`.
//! - **`sql-rules`** feature — what SQL a custom app may send to its own
//!   schema (`sql_rules`).
//! - **`import`** feature — implies `admin` and `connector`. Loads a CSV or
//!   Parquet file into a table in a workspace's tenant (`import`), reading it
//!   with DuckDB in this process.
//! - **`preview-sql`** feature — implies `sql-rules`. The AST rewrite and the
//!   independent verifier that keep a workspace preview's Airhouse writes in
//!   `preview_<key>__<schema>` (`preview_sql`), and the same overlay for a
//!   custom-app environment's sibling schema.
//! - **`preview-registry`** feature — implies `preview-sql`. The Postgres
//!   store of which tables an overlay holds (`preview_sql::registry`).

// `TenantImporter::import` awaits the provisioner and the broker inside one
// `async_trait` future, whose layout exceeds the default query depth.
#![recursion_limit = "256"]

#[cfg(feature = "connector")]
pub mod connector;

/// The schemas a custom app owns, and how a non-production environment's
/// sibling of one is named — pure, so every feature can hide the siblings.
pub mod app_schema;

/// The SQL a custom app may send to its own Airhouse schema.
#[cfg(feature = "sql-rules")]
pub mod sql_rules;

/// An app's statement moved from its own schema to a non-production sibling,
/// checked against both.
#[cfg(feature = "sql-rules")]
pub mod sql_retarget;

/// How SQL is parsed wherever a check depends on it: tokenized once, refused
/// when nested too deep, and parsed on a stack that holds the tree.
#[cfg(feature = "sql-rules")]
pub mod sql_parse;

/// An upper bound on a statement's tree depth, read from its tokens.
#[cfg(feature = "sql-rules")]
mod sql_depth;

/// The rewrite and verifier that keep a workspace preview's Airhouse writes in
/// the preview's own schemas.
#[cfg(feature = "preview-sql")]
pub mod preview_sql;

#[cfg(feature = "credentials")]
pub mod entity;

#[cfg(feature = "credentials")]
pub mod migration;

#[cfg(feature = "admin")]
pub mod admin;

#[cfg(feature = "admin")]
pub mod broker;

#[cfg(feature = "admin")]
pub mod config;

#[cfg(feature = "admin")]
pub mod local_seed;

#[cfg(feature = "admin")]
pub mod provisioner;

#[cfg(feature = "admin")]
pub mod post_provision;

/// Give a workspace its tenant when it has none (idempotent).
#[cfg(feature = "admin")]
pub mod ensure_tenant;

/// Remove the tenants of workspaces that are being deleted.
#[cfg(feature = "admin")]
pub mod workspace_delete;

/// Load a CSV or Parquet file into a table in a workspace's tenant.
#[cfg(feature = "import")]
pub mod import;

#[cfg(feature = "rest")]
pub mod api;

// ── Re-exports ────────────────────────────────────────────────────────────────

#[cfg(feature = "connector")]
pub use connector::AirhouseConnector;

#[cfg(feature = "admin")]
pub use admin::{
    AirhouseAdminClient, AirhouseError, CreatedServiceAccount, EphemeralCredential,
    ServiceAccountRecord, TenantRecord, TokenAuth, UserRecord, UserRole,
};

#[cfg(feature = "admin")]
pub use broker::{
    AirhouseTokenBroker, BrokerError, BrokerSubject, DEFAULT_EXTERNAL_TTL, DEFAULT_INTERNAL_TTL,
    SystemPurpose, airhouse_role_for,
};

#[cfg(feature = "admin")]
pub use config::{
    AIRHOUSE_ADMIN_TOKEN_VAR, AIRHOUSE_ANALYTICS_WIRE_HOST_VAR, AIRHOUSE_ANALYTICS_WIRE_PORT_VAR,
    AIRHOUSE_BASE_URL_VAR, AIRHOUSE_CATALOG_VAR, AIRHOUSE_WIRE_HOST_VAR, AIRHOUSE_WIRE_PORT_VAR,
    AirhouseConfig, AirhouseRuntimeConfig, LOCAL_ORG_ID, REQUIRED_VARS, WireEndpoint,
    analytics_wire_endpoint, ducklake_catalog, provisioner_for, token_broker, wire_endpoint,
};

#[cfg(feature = "admin")]
pub use post_provision::{HookError, PostProvisionHook, register_post_provision_hook};

#[cfg(feature = "admin")]
pub use local_seed::ensure_local_org_seeded;

#[cfg(feature = "admin")]
pub use provisioner::{
    DetachedTenant, ProvisionerError, RotatedServiceAccount, TenantProvisioner, TenantRemoval,
    tenant_name_for_workspace,
};

#[cfg(feature = "admin")]
pub use ensure_tenant::{EnsureTenantError, EnsuredTenant, ensure_tenant};
