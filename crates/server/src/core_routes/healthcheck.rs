use axum::{http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use utoipa::ToSchema;

use crate::build_identity::{self, BuildInfo};

#[derive(Serialize, Deserialize, ToSchema)]
pub struct HealthCheckResponse {
    pub status: String,
    pub timestamp: u64,
    pub service: String,
    /// The image tag this process runs as (`main-<sha>`); `null` outside an
    /// image, where `build_info.git_commit_short` names the build.
    pub image_tag: Option<String>,
    pub database: DatabaseStatus,
    pub build_info: BuildInfo,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct DatabaseStatus {
    pub connected: bool,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct VersionResponse {
    /// The image tag this process runs as (`main-<sha>`); `null` outside an
    /// image, where `build_info.git_commit_short` names the build.
    pub image_tag: Option<String>,
    pub service: String,
    pub build_info: BuildInfo,
}

// ── OPERATOR NOTE — deliberately `//`, not `///` ─────────────────────────────
//
// utoipa lifts a handler's doc comment into the OpenAPI operation description,
// and the spec is served unauthenticated at /apidoc. Anything written with ///
// below ships to every deployment's API docs, self-hosted customers included,
// so the incident narrative and our internal topology stay in plain comments.
//
// WHY THIS ENDPOINT IS NOT A LIVENESS PROBE
//
// /health returns 503 when Postgres is unreachable. Wired to livenessProbe,
// that makes kubelet restart the pod during a database incident — a crash loop
// layered on top of an outage, and worst on a single-replica StatefulSet with
// an RWO volume, where the restart also risks a Multi-Attach stall. oxy-prod
// did exactly this until 2026-08-11; the doc comment here previously listed
// "Kubernetes liveness/readiness probes" as an intended use, which is where
// the misconfiguration came from.
//
// The deployed paths are /api/health, /api/live and /api/ready — these routes
// are only mounted inside the router nested at /api (see `public_routes` in
// this module's parent, merged beside oxy-app's build_public_routes through
// SurfaceSeams::public, and the nest in serve.rs). The bare forms below match
// the #[utoipa::path]
// annotations, which are relative to the spec's `servers = ["/api"]`. A probe
// pointed at bare /live gets a 404, which kubelet counts as a failure — the
// same restart-during-incident outcome by a different route.
//
// A durable version of this guidance lives in internal-docs/, which is where
// whoever wires probes in the infrastructure repo will actually look.

/// Health check endpoint — for humans and uptime monitors.
///
/// Returns service health including database connectivity, plus the image tag
/// and build information. Unauthenticated. Served at `/api/health`.
///
/// Intended for external uptime monitoring (the build info aids triage) and for
/// operators asking "what is deployed, and can it reach its database?".
///
/// **Not a Kubernetes liveness probe** — it returns 503 when Postgres is
/// unreachable, which would restart the pod during a database incident. Use
/// [`liveness_check`] (`/live`, served at `/api/live`) for liveness and
/// [`readiness_check`] (`/ready`, served at `/api/ready`) for readiness and
/// load-balancer health.
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthCheckResponse),
        (status = 503, description = "Service is unhealthy", body = HealthCheckResponse)
    )
)]
pub async fn health_check()
-> Result<Json<HealthCheckResponse>, (StatusCode, Json<HealthCheckResponse>)> {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let db_status = check_database_connection().await;

    let status = if db_status.connected {
        "healthy"
    } else {
        "unhealthy"
    };

    let response = HealthCheckResponse {
        status: status.to_string(),
        timestamp,
        service: "oxy".to_string(),
        image_tag: build_identity::image_tag().map(str::to_string),
        database: db_status,
        build_info: build_identity::build_info(),
    };

    if status == "healthy" {
        Ok(Json(response))
    } else {
        Err((StatusCode::SERVICE_UNAVAILABLE, Json(response)))
    }
}

async fn check_database_connection() -> DatabaseStatus {
    match oxy::database::client::establish_connection().await {
        Ok(db) => {
            // Try a simple query to verify the connection is actually working
            match sea_orm::DatabaseConnection::ping(&db).await {
                Ok(_) => DatabaseStatus {
                    connected: true,
                    message: Some("Database connection successful".to_string()),
                },
                Err(e) => {
                    tracing::error!("Database ping failed: {}", e);
                    DatabaseStatus {
                        connected: false,
                        message: Some(format!("Database ping failed: {}", e)),
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to establish database connection: {}", e);
            DatabaseStatus {
                connected: false,
                message: Some(format!("Database connection failed: {}", e)),
            }
        }
    }
}

// Fleet-wide failure mode, worth knowing before relying on this: every replica
// pings the SAME Postgres, so a database outage fails readiness across the
// whole fleet at once and the Service drops to zero endpoints. That is intended
// — a replica that cannot read is not ready — but the resulting behaviour is
// "all traffic fails at the load balancer", not "traffic shifts to a healthy
// replica". There is no healthy replica to shift to.

/// Readiness check endpoint. Served at `/api/ready`.
///
/// Returns 200 only when the service can reach Postgres, and 503 while it
/// cannot — so an unready pod leaves the load balancer and the Service
/// endpoints **without being restarted**, and returns on its own the moment the
/// database recovers.
///
/// This is the correct target for `readinessProbe` and for load-balancer health
/// checks. For `livenessProbe` use [`liveness_check`] instead: liveness must not
/// depend on a database.
#[utoipa::path(
    get,
    path = "/ready",
    tag = "Health",
    responses(
        (status = 200, description = "Service is ready"),
        (status = 503, description = "Service is not ready")
    )
)]
pub async fn readiness_check() -> StatusCode {
    match check_database_connection().await {
        db_status if db_status.connected => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Liveness check endpoint
///
/// Minimal check to verify the service process is alive.
/// This endpoint always returns 200 if the service is running.
/// Designed for Kubernetes liveness probes.
#[utoipa::path(
    get,
    path = "/live",
    tag = "Health",
    responses(
        (status = 200, description = "Service is alive")
    )
)]
pub async fn liveness_check() -> StatusCode {
    StatusCode::OK
}

/// Version information endpoint
///
/// Returns the image tag this process runs as (`main-<sha>`, what a deploy
/// pins) and its build information, without any health checks. There is no
/// semver: the last published release says nothing about which image is
/// running.
/// This endpoint always returns 200 as long as the service is running,
/// making it reliable for displaying diagnostics even when the service is unhealthy.
#[utoipa::path(
    get,
    path = "/version",
    tag = "Health",
    responses(
        (status = 200, description = "Version information", body = VersionResponse)
    )
)]
pub async fn version_info() -> Json<VersionResponse> {
    Json(VersionResponse {
        image_tag: build_identity::image_tag().map(str::to_string),
        service: "oxy".to_string(),
        build_info: build_identity::build_info(),
    })
}
