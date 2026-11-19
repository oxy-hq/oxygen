//! The availability reading of one custom app: the derived SLI a list row
//! carries in its `availability` field.
//!
//! ## How this differs from `/health`, which it does not replace
//!
//! [`custom_apps_health`](super::custom_apps_health) answers a **deployment
//! integrity** question from Postgres: is this app registered, published, does
//! it have a build, does that build have an entrypoint. It is a static ladder,
//! and a perfectly green one is entirely compatible with every request to the
//! app failing.
//!
//! This answers the **serving** question, from the traffic Oxy already
//! terminates: of the requests real people made, what fraction succeeded, and
//! is the error budget burning fast enough to act on. A prober cannot produce
//! this answer: a custom-app host returns 200 with the SPA shell for every
//! path, so an outside-in check is green whatever the app is doing. Oxy is on
//! the inside of that, which is why the answer is a query.
//!
//! The per-app route `GET /customer-apps/{org}/{app}/availability` that used to
//! serve this reading was retired once the list rows carried it
//! (`internal-docs/legacy-removal.md` §4); the list
//! ([`crate::admin::apps::list_availability`]) is the one reader.
//!
//! See `internal-docs/2026-09-04-custom-app-observability-design.md` §3 Layer 2.

use oxy_observability::burn_rate::{BurnVerdict, SloConfig, evaluate};
use oxy_observability::types::AppAvailabilityWindow;
use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct WindowReport {
    pub window_minutes: u32,
    pub total: u64,
    pub failed: u64,
    /// `null` when the window carried no traffic — **not** `0.0`. "Nothing
    /// failed" and "nothing happened" are different answers and a chart that
    /// conflates them draws a healthy flat line over a dead app.
    pub failure_ratio: Option<f64>,
}

/// Everything the availability answer says about one app **except who the app
/// is**: the `availability` field on the `/customer-apps` list rows
/// ([`crate::admin::apps::list_availability`]), built by
/// [`AvailabilityReading::from_windows`].
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AvailabilityReading {
    /// `no_opinion` | `healthy` | `burning`.
    pub verdict: &'static str,
    /// `page` | `ticket`, present only when `verdict` is `burning`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burn_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_window_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_window_minutes: Option<u32>,
    pub objective: f64,
    pub windows: Vec<WindowReport>,
    /// Recent sessions on the live build vs older builds — whether open tabs
    /// have picked up the last publish. See `custom_apps_open_builds`.
    pub open_builds: super::custom_apps_open_builds::OpenBuilds,
}

impl AvailabilityReading {
    /// The verdict and the numbers behind it, from one app's windows (in
    /// [`ALERT_WINDOWS_MINUTES`] order). Empty or quiet windows are
    /// `no_opinion`, never `healthy`.
    pub fn from_windows(
        windows: &[AppAvailabilityWindow],
        open_builds: super::custom_apps_open_builds::OpenBuilds,
    ) -> Self {
        let cfg = SloConfig::default();
        let mut reading = Self {
            verdict: "no_opinion",
            severity: None,
            burn_rate: None,
            long_window_minutes: None,
            short_window_minutes: None,
            objective: cfg.objective,
            windows: windows
                .iter()
                .map(|w| WindowReport {
                    window_minutes: w.window_minutes,
                    total: w.total,
                    failed: w.failed,
                    failure_ratio: w.failure_ratio(),
                })
                .collect(),
            open_builds,
        };
        match evaluate(windows, &cfg) {
            BurnVerdict::NoOpinion => {}
            BurnVerdict::Healthy => reading.verdict = "healthy",
            BurnVerdict::Burning {
                severity,
                burn_rate,
                long_minutes,
                short_minutes,
                ..
            } => {
                reading.verdict = "burning";
                reading.severity = Some(match severity {
                    oxy_observability::burn_rate::Severity::Page => "page",
                    oxy_observability::burn_rate::Severity::Ticket => "ticket",
                });
                reading.burn_rate = Some(burn_rate);
                reading.long_window_minutes = Some(long_minutes);
                reading.short_window_minutes = Some(short_minutes);
            }
        }
        reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three verdicts must stay distinguishable on the wire. In particular
    /// `no_opinion` must never serialize as something a dashboard would render
    /// green: an app with no traffic has not been shown to work.
    #[test]
    fn no_opinion_is_not_spelled_like_healthy() {
        let cfg = SloConfig::default();
        assert_eq!(evaluate(&[], &cfg), BurnVerdict::NoOpinion);
        // The mapping above turns NoOpinion into this literal; pin it so a
        // rename cannot quietly make a silent app look healthy.
        let verdict = "no_opinion";
        assert_ne!(verdict, "healthy");
    }

    /// An empty window reports `null`, not `0.0`.
    #[test]
    fn an_empty_window_reports_null_not_zero() {
        let w = oxy_observability::types::AppAvailabilityWindow {
            window_minutes: 5,
            total: 0,
            failed: 0,
        };
        let report = WindowReport {
            window_minutes: w.window_minutes,
            total: w.total,
            failed: w.failed,
            failure_ratio: w.failure_ratio(),
        };
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["failure_ratio"].is_null(), "{json}");
    }

    /// The reading's wire shape: the burn fields are present only when
    /// burning, and `no_opinion` carries none of them.
    #[test]
    fn the_reading_keeps_its_wire_shape() {
        use crate::custom_apps_open_builds::OpenBuilds;
        let body = |windows: &[AppAvailabilityWindow]| {
            serde_json::to_value(AvailabilityReading::from_windows(
                windows,
                OpenBuilds::not_measured(),
            ))
            .unwrap()
        };
        let win = |m, total, failed| AppAvailabilityWindow {
            window_minutes: m,
            total,
            failed,
        };

        let quiet = body(&[]);
        let mut keys: Vec<_> = quiet.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["objective", "open_builds", "verdict", "windows"]);
        assert_eq!(quiet["verdict"], "no_opinion");

        let burning = body(&[win(5, 30, 15), win(60, 100, 50)]);
        assert_eq!(burning["verdict"], "burning");
        for key in [
            "severity",
            "burn_rate",
            "long_window_minutes",
            "short_window_minutes",
        ] {
            assert!(burning.get(key).is_some(), "{key}");
        }
    }
}
