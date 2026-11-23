//! The hourly audit chain anchor loop: once per interval across the fleet.
//!
//! Everything it runs (the configuration, the anchor write, the verifier) is
//! `oxy_app_core::audit::anchor` since the admin split's PR 3. The loop stays
//! here because it builds the custom-app store's S3 client
//! (`custom_apps_storage::s3::client`, one builder for the MinIO path-style
//! flag), which `oxy-app-core` sits below.

use aws_sdk_s3::Client as S3Client;
use sea_orm::DatabaseConnection;

use oxy_app_core::audit::anchor::{ANCHOR_JOB, AnchorConfig, AnchorError, BUCKET_ENV, anchor_once};

/// Spawn the hourly anchor loop: once per interval across the fleet. Every
/// replica that runs singleton work starts it and the
/// [`fleet_claim`](oxy_app_core::fleet_claim) row picks one per interval —
/// every replica anchoring would write an object per replica per hour under a
/// compliance lock nobody can delete early, and two in the same minute would
/// fail the second's `if-none-match` as a spurious error.
pub fn spawn_audit_anchor_loop(
    db: DatabaseConnection,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let Some(cfg) = AnchorConfig::from_env() else {
        tracing::info!(
            "audit anchor: {BUCKET_ENV} not set; chain heads are not anchored outside the database"
        );
        return;
    };
    tokio::spawn(async move {
        let s3 = oxy_custom_apps::storage::s3::client().await;
        let interval = cfg.interval;
        let (pass_db, cfg, s3) = (
            db.clone(),
            std::sync::Arc::new(cfg),
            std::sync::Arc::new(s3),
        );
        // A due claim anchors at the first poll: a fresh deployment should have
        // an anchor within seconds, not an hour.
        oxy_app_core::fleet_claim::run_claimed(
            db,
            ANCHOR_JOB,
            interval,
            shutdown.cancelled_owned(),
            move || {
                let (db, cfg, s3) = (pass_db.clone(), cfg.clone(), s3.clone());
                async move { anchor_and_log(&db, &s3, &cfg).await }
            },
        )
        .await;
    });
}

async fn anchor_and_log(db: &DatabaseConnection, s3: &S3Client, cfg: &AnchorConfig) {
    match anchor_once(db, s3, cfg).await {
        Ok(o) => tracing::info!(
            key = %o.key, heads = o.heads, sha256 = %o.sha256,
            retain_until = %o.retain_until, "audit anchor written"
        ),
        Err(AnchorError::AlreadyAnchored(key)) => tracing::info!(
            key = %key, "audit anchor: this minute is already anchored; skipping"
        ),
        Err(e) => tracing::error!(error = %e, "audit anchor failed"),
    }
}
