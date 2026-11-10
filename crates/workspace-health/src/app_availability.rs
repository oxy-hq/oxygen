//! Custom-app availability as a workspace-health dimension.
//!
//! Named `app_availability`, not `custom_apps`: the custom-apps boundary test
//! (`tests/custom_apps/custom_apps_boundary.rs`) selects files by NAME, so a
//! `custom_apps.rs` anywhere in the tree is scanned as part of that surface —
//! and this one legitimately imports its own parent module's `HealthStatus`.
//! The rename is also the better name: this is the availability dimension,
//! which happens to be about custom apps.
//!
//! ## Why this hangs off Workspace Health rather than alerting on its own
//!
//! `burn_rate::evaluate` decides *whether* an app is burning its error budget.
//! Turning that into a page needs everything around it: a transition rule so a
//! continuously-broken app does not alert every pass, a re-alert interval, a
//! clear when it recovers, and somewhere for the message to go. Workspace Health
//! already has all of that, tested, in `alert.rs`. Rebuilding it here would mean
//! a second, subtly different pager for operators to learn.
//!
//! ## Per-app burn, per-workspace status
//!
//! The two grains do not match: availability is per app, Workspace Health is per
//! workspace. The roll-up is **worst-app-wins**, the same rule the smoke and
//! reconciliation dimensions use for their verdict lists, and the reason names
//! the worst app and counts the rest so a workspace with eight broken apps does
//! not hide seven of them behind one line.
//!
//! ## The severity mapping is the alerting policy
//!
//! | Burn verdict | Health status | Effect |
//! | --- | --- | --- |
//! | `Burning { Page }` | `Unhealthy` | **Pages Slack** on transition |
//! | `Burning { Ticket }` | `Degraded` | Shows in the admin table, never pages |
//! | `Healthy` / `NoOpinion` | `Healthy` | Clears |
//!
//! That mapping is doing real work. Workspace Health only pages on an *unhealthy*
//! transition and deliberately never on "degraded" — so routing the ticket-grade
//! rule to `Degraded` is what keeps a slow 4%-failure leak off the pager while
//! still putting it in front of an operator. It also means the two severity
//! systems agree instead of each having an opinion about the same incident.
//!
//! ## Silence is not health
//!
//! `NoOpinion` — an app with no traffic, or below the burn evaluator's traffic
//! floor — maps to `Healthy` *for the dimension*, because a dimension has no way
//! to express "unknown" and reporting it as broken would page for every app
//! nobody used overnight. This is the honest limit of putting availability on
//! this ladder, and it is why the per-app endpoint keeps `no_opinion` as a
//! distinct verdict: the admin surface can tell "quiet" from "fine", the pager
//! cannot.

use entity::apps;
use oxy_observability::burn_rate::{
    ALERT_WINDOWS_MINUTES, BurnVerdict, Severity, SloConfig, evaluate as evaluate_burn,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::evaluator::HealthStatus;

/// One app's availability verdict, in the shape the dimension rolls up.
#[derive(Debug, Clone)]
pub struct AppAvailabilityVerdict {
    pub app_id: Uuid,
    pub app_slug: String,
    pub status: HealthStatus,
    pub reason: Option<String>,
}

/// Map a burn verdict onto the workspace-health ladder. Pure — this is the
/// alerting policy and it is unit-tested as such.
pub fn status_for(app_slug: &str, verdict: &BurnVerdict) -> (HealthStatus, Option<String>) {
    match verdict {
        // Deliberately Healthy, not a third state. See the module docs: a
        // dimension cannot say "unknown", and paging for every app nobody used
        // last night is worse than saying nothing about it.
        BurnVerdict::NoOpinion | BurnVerdict::Healthy => (HealthStatus::Healthy, None),
        BurnVerdict::Burning {
            severity,
            burn_rate,
            long_minutes,
            failure_ratio,
            ..
        } => {
            let status = match severity {
                Severity::Page => HealthStatus::Unhealthy,
                Severity::Ticket => HealthStatus::Degraded,
            };
            let reason = format!(
                "{app_slug}: {:.0}% of requests failing over {}m ({:.1}× error budget)",
                failure_ratio * 100.0,
                long_minutes,
                burn_rate
            );
            (status, Some(reason))
        }
    }
}

/// Evaluate every published app in the workspace.
///
/// Returns empty — which reads as a clear dimension — when observability is not
/// configured. That is the default for a developer's `oxy serve`, and it must be
/// silence rather than a failing dimension: "we are not measuring" is not "it is
/// broken", and a workspace-health table that went red on every dev box would be
/// ignored everywhere.
pub async fn gather(db: &DatabaseConnection, workspace_id: Uuid) -> Vec<AppAvailabilityVerdict> {
    let Some(store) = oxy_observability::global::get_global() else {
        return Vec::new();
    };

    // Only published apps. An unpublished one serves nobody, so it would score
    // `NoOpinion` anyway — filtering here just avoids the round-trip per draft.
    let apps = match apps::Entity::find()
        .filter(apps::Column::ProjectId.eq(workspace_id))
        .filter(apps::Column::PublishedAt.is_not_null())
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("custom-app availability: app lookup failed for {workspace_id}: {e}");
            return Vec::new();
        }
    };

    let cfg = SloConfig::default();
    let mut out = Vec::with_capacity(apps.len());
    for app in apps {
        let windows = match store
            .get_app_availability(
                &app.org_id.to_string(),
                &app.id.to_string(),
                ALERT_WINDOWS_MINUTES,
            )
            .await
        {
            Ok(w) => w,
            Err(e) => {
                // A store blip must not turn into a page. Skipping leaves the
                // app out of the roll-up entirely, which reads as "no signal"
                // rather than as either verdict — the same posture every other
                // dimension takes when its source is unavailable.
                tracing::warn!(
                    "custom-app availability query failed for app {}: {e}",
                    app.id
                );
                continue;
            }
        };
        let verdict = evaluate_burn(&windows, &cfg);
        let (status, reason) = status_for(&app.slug, &verdict);
        out.push(AppAvailabilityVerdict {
            app_id: app.id,
            app_slug: app.slug,
            status,
            reason,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burning(severity: Severity) -> BurnVerdict {
        BurnVerdict::Burning {
            severity,
            burn_rate: 15.0,
            long_minutes: 60,
            short_minutes: 5,
            failure_ratio: 0.15,
        }
    }

    /// The page-grade rule is the only one that reaches the pager, because
    /// Workspace Health alerts on an unhealthy transition and never on degraded.
    #[test]
    fn a_page_grade_burn_maps_to_unhealthy_so_it_reaches_slack() {
        let (status, reason) = status_for("orders", &burning(Severity::Page));
        assert_eq!(status, HealthStatus::Unhealthy);
        assert!(
            reason.unwrap().contains("orders"),
            "reason must name the app"
        );
    }

    /// A slow leak belongs in front of an operator, not on a pager at 3am.
    #[test]
    fn a_ticket_grade_burn_maps_to_degraded_which_never_pages() {
        let (status, _) = status_for("orders", &burning(Severity::Ticket));
        assert_eq!(status, HealthStatus::Degraded);
    }

    /// An app nobody used must not page. This is the deliberate lossy edge of
    /// the mapping — documented in the module docs, and the reason the per-app
    /// endpoint keeps `no_opinion` as its own verdict.
    #[test]
    fn a_quiet_app_does_not_page() {
        let (status, reason) = status_for("orders", &BurnVerdict::NoOpinion);
        assert_eq!(status, HealthStatus::Healthy);
        assert!(reason.is_none());
    }

    #[test]
    fn a_healthy_app_clears() {
        let (status, reason) = status_for("orders", &BurnVerdict::Healthy);
        assert_eq!(status, HealthStatus::Healthy);
        assert!(reason.is_none());
    }

    /// The reason line has to carry enough to act on without opening a
    /// dashboard: which app, how bad, over what window.
    #[test]
    fn the_reason_carries_app_ratio_and_window() {
        let reason = status_for("orders", &burning(Severity::Page)).1.unwrap();
        assert!(reason.contains("orders"), "{reason}");
        assert!(reason.contains("15%"), "{reason}");
        assert!(reason.contains("60m"), "{reason}");
        assert!(reason.contains("15.0×"), "{reason}");
    }
}
