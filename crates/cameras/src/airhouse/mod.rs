//! Airhouse-side writer + schema management for the camera fleet.
//!
//! Three concerns live here:
//!
//! 1. **Schema DDL** (`schema`) — `CREATE TABLE IF NOT EXISTS` for the
//!    per-tenant tables (`oxy_cam_events`, `oxy_cam_camera_health`,
//!    `oxy_cam_box_health`, `oxy_cam_compliance_reports`, `oxy_cam_device_logs`).
//! 2. **Connections** (`client`) — **persistent, per-tenant** pgwire clients,
//!    now `airhouse::tenant` (re-exported here for one release).
//!    Cameras are tenant-based (each workspace is its own Airhouse tenant with
//!    its own minted credentials), so the [`client`] registry keeps one
//!    long-lived `tokio_postgres::Client` per `(workspace_id, role, purpose)`
//!    and reuses it across writes. This is the fix for the edge-ingest session
//!    churn that OOM'd Airhouse: a fresh connection per write spawned a fresh
//!    server-side DuckDB session each time. The ephemeral credential is still
//!    minted per-tenant via the SA broker (`SystemPurpose::EdgeIngest`).
//! 3. **Ensure cache** — a lazy "we've already created the tables in this
//!    tenant" set so the DDL only runs on the first ingest per
//!    (workspace_id, process lifetime).
//!
//! Service-layer entry points (`service::ingest`, `service::compliance`)
//! call `connect_and_ensure(workspace_id)` to get a ready-to-use, reused
//! [`TenantClient`], then build a multi-row INSERT via simple-query (DuckLake
//! doesn't speak prepared statements / `$N` placeholders).

pub mod schema;

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use tokio::sync::RwLock;
use tokio_postgres::Client;
use uuid::Uuid;

use airhouse::{SystemPurpose, UserRole};

// The generic tenant client lives in `airhouse::tenant` (crate-tiers stage
// D1). Re-exported at its old paths for one release, so
// `oxy_cameras::airhouse::{client, escape, connect, ..}` keep resolving; the
// leading `::` names the crate, not this module.
pub use ::airhouse::tenant::{
    AirhouseError, TenantClient, client, connect, connect_timeout, escape, ingest_ttl,
    insert_chunk_rows, is_missing_table, max_reconnect_attempts, pg_error_text, read_ttl,
};

/// In-process record of which workspace tenants have had the
/// camera-fleet DDL applied this process lifetime. Keyed by
/// `workspace_id`. Eviction happens only on process restart; that's
/// fine because the DDL is idempotent (`IF NOT EXISTS`).
fn ensured() -> &'static RwLock<HashSet<Uuid>> {
    static C: OnceLock<RwLock<HashSet<Uuid>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Ensure the camera-fleet DDL has been applied to this workspace's tenant at
/// least once this process, then return the **reused** Writer ingest client.
///
/// **Where the DDL actually runs:** in production, the
/// `airhouse::register_post_provision_hook` registered at app startup
/// (see `crates/server/src/commands/serve.rs`) fires this same
/// `ensure_schema` from `TenantProvisioner::provision`, so by the time
/// any edge box hits us the tables already exist. This function's
/// `ensured()` short-circuit + idempotent `CREATE TABLE IF NOT EXISTS`
/// then makes the ingest path's contribution effectively zero on the
/// hot path.
///
/// **The fallback case:** kept for safety in two scenarios:
///   1. Tenants provisioned before the hook was registered (one-time —
///      solved automatically the first time ingest runs against any such
///      tenant).
///   2. A post-provision hook failure (logged but non-fatal — see
///      `airhouse::post_provision::invoke_all`).
///
/// **Role split**: DDL runs with **Admin** (Writer cannot `CREATE TABLE` in
/// DuckLake — fails with `42501 Permission denied`). INSERTs run with
/// **Writer** (least privilege for ingest). The two roles are separate
/// persistent connections (and separate broker cache keys), so the Writer
/// reuse doesn't churn the Admin path.
pub async fn connect_and_ensure(workspace_id: Uuid) -> Result<Arc<TenantClient>, AirhouseError> {
    // Fast path: schema already ensured for this workspace this process.
    let already_ensured = ensured().read().await.contains(&workspace_id);
    if !already_ensured {
        ensure_schema(workspace_id).await?;
        ensured().write().await.insert(workspace_id);
    }
    client::tenant_client(
        workspace_id,
        UserRole::Writer,
        SystemPurpose::EdgeIngest,
        ingest_ttl(),
        client::Pool::Serving,
    )
    .await
}

/// Read-side companion to [`connect_and_ensure`]. Used by service-layer
/// SELECTs (e.g. the Compliance tab pulling reports for a camera).
///
/// Differences from the write path:
///   - Audited as [`SystemPurpose::ComplianceReportsRead`] so the audit log
///     can separate UI traffic from bulk edge ingest.
///   - Mints with [`UserRole::Reader`] — read-only, can SELECT but not
///     INSERT / DDL.
///   - Shorter credential TTL ([`read_ttl`]).
///   - Does NOT run schema DDL. If the table doesn't exist (no edge box ever
///     wrote to it), the SELECT just returns an empty set or errors as
///     `undefined_table`, both of which are accurate.
///   - Routed to the **analytics** DP pool ([`client::Pool::Analytics`]) when
///     one is configured. Every camera dashboard read — health-summary,
///     compliance, cost, logs, rollup — funnels through here, so this is the
///     one place that decides it.
///
/// Why the analytics pool: these are UI SELECTs that scan DuckLake, and they
/// were sharing the serving DP with edge ingest. That coupled them in both
/// directions — dashboard scans competing with hot writes, and, worse, a
/// write-path fault taking the dashboard down with it. Each DP has its own
/// circuit breaker: on 2026-07-15 rejected `oxy_cam_*` writes tripped the
/// serving DP's breaker, and because reads lived on that same pool the
/// dashboard went down with the ingest. On the analytics pool a write-path
/// fault costs writes only.
///
/// Falls back to the serving endpoint when `AIRHOUSE_ANALYTICS_WIRE_HOST` is
/// unset, so deployments without a separate analytics pool are unchanged.
///
/// Like the write path, the underlying connection is **persistent and reused**
/// per tenant rather than opened per request.
pub async fn connect_for_reads(workspace_id: Uuid) -> Result<Arc<TenantClient>, AirhouseError> {
    client::tenant_client(
        workspace_id,
        UserRole::Reader,
        SystemPurpose::ComplianceReportsRead,
        read_ttl(),
        client::Pool::Analytics,
    )
    .await
}

/// One-shot connection for the log-retention sweep.
///
/// Retention runs infrequently (hourly+, `OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS`)
/// and issues large `DELETE`s. Deliberately **not** routed through the
/// persistent `(Writer, EdgeIngest)` ingest connection: the server-side DuckDB
/// session executes serially, so a slow retention `DELETE` sharing that
/// connection could head-of-line-block live edge ingest for the tenant. A
/// dedicated one-shot connection (audited under [`SystemPurpose::Scheduler`])
/// isolates it; being one-shot is also cheaper than holding a second persistent
/// session per tenant for an operation that runs a couple times a day. No DDL —
/// a missing table is handled by the caller as "nothing to retain".
pub async fn connect_for_retention(workspace_id: Uuid) -> Result<Client, AirhouseError> {
    connect(
        workspace_id,
        UserRole::Writer,
        SystemPurpose::Scheduler,
        ingest_ttl(),
    )
    .await
}

/// One-shot Admin-credentialled DDL run. The Admin client is created, used
/// for the `CREATE TABLE IF NOT EXISTS` statements, and dropped immediately —
/// no long-lived Admin handle in process memory. DDL is rare (gated by
/// `ensured()`), so this path intentionally does **not** join the persistent
/// registry.
pub async fn ensure_schema(workspace_id: Uuid) -> Result<(), AirhouseError> {
    let admin_client = connect(
        workspace_id,
        UserRole::Admin,
        SystemPurpose::EdgeIngest,
        ingest_ttl(),
    )
    .await?;
    schema::ensure(&admin_client).await?;
    drop(admin_client);
    Ok(())
}
