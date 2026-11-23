//! The Stripe seat reconciler, disabled since before decompose BL1 and carried
//! over unchanged from `oxy-app`'s `router::entry` (2026-11-23, the user's
//! decision: kept, not deleted). Nothing calls it; `oxy-app`'s
//! `entry_boot_guard_tests` still fails if a router builder ever does.

use std::time::Duration;

/// 6-hour background loop that reconciles Stripe seat quantity for every
/// paid org, catching any drift between member counts and what we last sent
/// to Stripe. Idempotent; silently does nothing if Stripe isn't configured.
#[allow(dead_code)] // Disabled for now; will re-enable later.
async fn spawn_billing_reconciler() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        // Already spawned (e.g. tests or duplicated router builds).
        return;
    }
    let Ok(svc) = oxy_billing::billing_service().await else {
        tracing::debug!("billing reconciler not spawned — Stripe isn't configured");
        return;
    };
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(6 * 60 * 60));
        // Skip the immediate tick — reconciliation at boot is redundant with
        // the live sync that just fired during any recent member change.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = svc.reconcile_all_seats().await {
                tracing::warn!(?e, "billing seat reconciliation failed");
            }
        }
    });
}
