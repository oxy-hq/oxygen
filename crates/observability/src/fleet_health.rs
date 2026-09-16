//! One app's health, in a shape that can admit ignorance.
//!
//! Pure: combines a [`burn_rate`](crate::burn_rate) verdict, a
//! [`heartbeat`](crate::heartbeat) verdict and the raw windows into the single
//! value a fleet table renders. No I/O, no clock.
//!
//! ## Why a new enum rather than reusing `HealthStatus`
//!
//! Workspace Health's ladder is `Healthy | Degraded | Unhealthy`, and
//! `app_availability` maps both "no traffic" and "below the floor" onto
//! `Healthy` — deliberately, because a *dimension* has no way to say "unknown"
//! and paging for every app nobody used overnight is worse than saying nothing.
//! That mapping is right for a pager and wrong for a table: it is exactly how a
//! dead app, an unmeasured app and a working app become the same green tick.
//!
//! [`BurnVerdict::NoOpinion`] already
//! carries the instruction — *"Callers should render this as 'no data', never as
//! a green tick"* — and until this module existed no caller honoured it, because
//! no caller had a value to render it as. [`AppHealth`] is that value.
//!
//! ## The two states that are the point
//!
//! - [`AppHealth::Quiet`] — real traffic, below the floor the ratio rules need.
//! - [`AppHealth::NotMeasured`] — capture is off, the workspace was never
//!   evaluated, or the query failed. **We do not know.**
//!
//! The Google SRE workbook draws this distinction and insists on it: *"Zero
//! traffic is an absence of request evidence, while low traffic is a coarse but
//! real signal. These two conditions should be treated explicitly instead of
//! hiding them behind a default value."* Both are hidden behind a default value
//! today, and that default is green.

use serde::Serialize;

use crate::burn_rate::{BurnVerdict, Severity};
use crate::heartbeat::Heartbeat;
use crate::types::AppAvailabilityWindow;

/// What a fleet table shows for one app.
///
/// Ordered worst-first so a fleet roll-up can sort on it and a reader's eye
/// lands on the apps that need them. `NotMeasured` sorts above `Quiet`: not
/// knowing is more actionable than knowing it is idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppHealth {
    /// A page-grade burn, or the low-traffic absolute rule matched.
    Down,
    /// A ticket-grade burn, or an established app that has gone silent.
    /// Visible; never pages.
    Degraded,
    /// Capture is off, the workspace is not evaluated, or the query failed.
    /// **Not health** — the absence of an answer.
    NotMeasured,
    /// Real traffic, below the floor the ratio rules need to have an opinion.
    Quiet,
    /// Burning error budget no faster than the SLO allows.
    Operational,
}

impl AppHealth {
    /// Whether this verdict is one an operator should look at. `Quiet` is not —
    /// most of the fleet is quiet most of the time, and a table where every row
    /// needs attention is a table nobody reads.
    pub fn needs_attention(self) -> bool {
        matches!(self, Self::Down | Self::Degraded | Self::NotMeasured)
    }
}

/// One app's verdict and the sentence explaining it.
#[derive(Debug, Clone, Serialize)]
pub struct AppVerdict {
    pub health: AppHealth,
    /// Why, in a line an operator can act on without opening anything else:
    /// which rule, how bad, over what window. `None` only for `Operational`.
    pub reason: Option<String>,
}

impl AppVerdict {
    /// The verdict for an app whose measurement never happened. `why` names the
    /// layer that was missing — capture off, workspace not evaluated, query
    /// failed — because those have different fixes and the table is useless if
    /// it cannot tell them apart.
    pub fn not_measured(why: impl Into<String>) -> Self {
        Self {
            health: AppHealth::NotMeasured,
            reason: Some(why.into()),
        }
    }
}

/// Classify one **measured** app.
///
/// [`AppHealth::NotMeasured`] is never produced here: "the query never ran" is
/// the caller's fact, not something these inputs can express. Use
/// [`AppVerdict::not_measured`] for it.
pub fn classify(
    windows: &[AppAvailabilityWindow],
    burn: &BurnVerdict,
    heartbeat: Heartbeat,
) -> AppVerdict {
    match burn {
        BurnVerdict::Burning {
            severity,
            burn_rate,
            long_minutes,
            failure_ratio,
            ..
        } => AppVerdict {
            health: match severity {
                Severity::Page => AppHealth::Down,
                Severity::Ticket => AppHealth::Degraded,
            },
            reason: Some(format!(
                "{:.0}% of requests failing over {long_minutes}m ({burn_rate:.1}× error budget)",
                failure_ratio * 100.0
            )),
        },
        // Checked after burn so a burning app is reported as burning. The two
        // cannot both hold in practice — silence means no requests, and a burn
        // needs them — but the precedence should not depend on that.
        BurnVerdict::Healthy | BurnVerdict::NoOpinion
            if matches!(heartbeat, Heartbeat::Silent { .. }) =>
        {
            let Heartbeat::Silent { expected } = heartbeat else {
                unreachable!("guarded by the match arm")
            };
            AppVerdict {
                health: AppHealth::Degraded,
                reason: Some(format!(
                    "no requests in {}h; {expected} in the same window a week ago",
                    crate::heartbeat::WINDOW_MINUTES / 60
                )),
            }
        }
        BurnVerdict::Healthy => AppVerdict {
            health: AppHealth::Operational,
            reason: None,
        },
        BurnVerdict::NoOpinion => AppVerdict {
            health: AppHealth::Quiet,
            reason: Some(quiet_reason(windows)),
        },
    }
}

/// Why an app is quiet — distinguishing "served nothing" from "served a little".
///
/// Both are `Quiet` on the badge, because both mean the ratio rules cannot
/// speak, but they are different situations and the difference is free to carry
/// in the sentence.
fn quiet_reason(windows: &[AppAvailabilityWindow]) -> String {
    let day = windows
        .iter()
        .max_by_key(|w| w.window_minutes)
        .map(|w| (w.window_minutes, w.total));
    match day {
        Some((_, 0)) | None => "no traffic to measure".to_string(),
        Some((minutes, total)) => format!(
            "{total} request(s) in {}h — below the {} needed to judge a failure rate",
            minutes / 60,
            crate::burn_rate::SloConfig::default().min_requests
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows(total: u64) -> Vec<AppAvailabilityWindow> {
        vec![
            AppAvailabilityWindow {
                window_minutes: 360,
                total: total / 4,
                failed: 0,
            },
            AppAvailabilityWindow {
                window_minutes: 1440,
                total,
                failed: 0,
            },
        ]
    }

    fn burning(severity: Severity) -> BurnVerdict {
        BurnVerdict::Burning {
            severity,
            burn_rate: 15.0,
            long_minutes: 60,
            short_minutes: 5,
            failure_ratio: 0.15,
        }
    }

    /// The whole reason this module exists: an app nobody measured must not be
    /// reported as working. This is the assertion that fails if someone
    /// "simplifies" `NotMeasured` into `Operational`.
    #[test]
    fn an_unmeasured_app_is_not_operational() {
        let v = AppVerdict::not_measured("observability capture is not configured");
        assert_eq!(v.health, AppHealth::NotMeasured);
        assert!(v.health.needs_attention());
        assert!(v.reason.is_some(), "the missing layer must be named");
    }

    /// Below the floor is its own state. Reporting it green is how a dead
    /// low-traffic app goes unnoticed, which is most of this fleet.
    #[test]
    fn a_quiet_app_is_quiet_not_operational() {
        let v = classify(&windows(12), &BurnVerdict::NoOpinion, Heartbeat::NoBaseline);
        assert_eq!(v.health, AppHealth::Quiet);
        assert_ne!(v.health, AppHealth::Operational);
    }

    /// Quiet is not an alarm — most apps are quiet most of the time.
    #[test]
    fn quiet_does_not_demand_attention_but_unmeasured_does() {
        assert!(!AppHealth::Quiet.needs_attention());
        assert!(AppHealth::NotMeasured.needs_attention());
    }

    /// Zero traffic and a little traffic are both `Quiet`, but the sentence has
    /// to tell them apart — they are different problems.
    #[test]
    fn the_quiet_reason_distinguishes_no_traffic_from_low_traffic() {
        let none = classify(&windows(0), &BurnVerdict::NoOpinion, Heartbeat::NoBaseline);
        assert!(none.reason.unwrap().contains("no traffic"));
        let some = classify(&windows(12), &BurnVerdict::NoOpinion, Heartbeat::NoBaseline);
        let reason = some.reason.unwrap();
        assert!(reason.contains("12"), "{reason}");
        assert!(reason.contains("20"), "the floor must be named: {reason}");
    }

    /// Silence against an established baseline is degraded — visible, never a
    /// page. See the heartbeat module for why it is not `Down`.
    #[test]
    fn silence_is_degraded_and_names_the_baseline() {
        let v = classify(
            &windows(0),
            &BurnVerdict::NoOpinion,
            Heartbeat::Silent { expected: 200 },
        );
        assert_eq!(v.health, AppHealth::Degraded);
        let reason = v.reason.unwrap();
        assert!(reason.contains("200"), "{reason}");
        assert!(reason.contains("6h"), "the window must be named: {reason}");
    }

    /// A burning app is reported as burning even if the heartbeat also has an
    /// opinion — the failure with a cause beats the failure without one.
    #[test]
    fn a_burn_outranks_silence() {
        let v = classify(
            &windows(500),
            &burning(Severity::Page),
            Heartbeat::Silent { expected: 200 },
        );
        assert_eq!(v.health, AppHealth::Down);
    }

    #[test]
    fn severity_maps_page_to_down_and_ticket_to_degraded() {
        assert_eq!(
            classify(&windows(500), &burning(Severity::Page), Heartbeat::Beating).health,
            AppHealth::Down
        );
        assert_eq!(
            classify(
                &windows(500),
                &burning(Severity::Ticket),
                Heartbeat::Beating
            )
            .health,
            AppHealth::Degraded
        );
    }

    #[test]
    fn a_healthy_app_is_operational_with_nothing_to_explain() {
        let v = classify(&windows(500), &BurnVerdict::Healthy, Heartbeat::Beating);
        assert_eq!(v.health, AppHealth::Operational);
        assert!(v.reason.is_none());
    }

    /// The fleet table sorts on this, so the order is load-bearing: the apps
    /// that need someone come first, and "we don't know" outranks "it's idle".
    #[test]
    fn worst_sorts_first_and_unmeasured_outranks_quiet() {
        let mut all = vec![
            AppHealth::Operational,
            AppHealth::Quiet,
            AppHealth::NotMeasured,
            AppHealth::Degraded,
            AppHealth::Down,
        ];
        all.sort();
        assert_eq!(
            all,
            vec![
                AppHealth::Down,
                AppHealth::Degraded,
                AppHealth::NotMeasured,
                AppHealth::Quiet,
                AppHealth::Operational,
            ]
        );
    }
}
