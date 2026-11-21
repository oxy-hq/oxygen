//! The list row's `availability` is what a per-app read of the same windows gives.
//!
//! One fake store stands in for ClickHouse and both reads go through it: a
//! per-app read of each app's windows, and the grouped ones the list makes. Each test runs in its own process under `cargo nextest` (the project's
//! runner), which the global store these tests install relies on.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use oxy_observability::burn_rate::ALERT_WINDOWS_MINUTES;
use oxy_observability::scope::WorkspaceScope;
use oxy_observability::store::ObservabilityStore;
use oxy_observability::types::*;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

use super::*;
use crate::custom_apps_availability::AvailabilityReading;

/// ClickHouse, as far as availability goes: per-app window counts and per-app
/// build sessions, read per app or grouped, and a count of the grouped reads.
#[derive(Debug, Default)]
struct FakeStore {
    windows: Mutex<HashMap<String, Vec<AppAvailabilityWindow>>>,
    sessions: Mutex<HashMap<String, Vec<AppBuildSessions>>>,
    fleet_reads: AtomicUsize,
    session_reads: AtomicUsize,
}

impl FakeStore {
    /// Zeroed windows for an app nothing was recorded for, as the real store
    /// answers for an app that served nothing.
    fn windows_of(&self, app: &str) -> Vec<AppAvailabilityWindow> {
        self.windows
            .lock()
            .unwrap()
            .get(app)
            .cloned()
            .unwrap_or_else(|| {
                ALERT_WINDOWS_MINUTES
                    .iter()
                    .map(|m| win(*m, 0, 0))
                    .collect()
            })
    }
}

fn win(window_minutes: u32, total: u64, failed: u64) -> AppAvailabilityWindow {
    AppAvailabilityWindow {
        window_minutes,
        total,
        failed,
    }
}

#[async_trait]
impl ObservabilityStore for FakeStore {
    async fn list_traces(
        &self,
        _: &WorkspaceScope,
        _: i64,
        _: i64,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<(Vec<TraceRow>, i64), OxyError> {
        unimplemented!()
    }
    async fn get_trace_detail(
        &self,
        _: &WorkspaceScope,
        _: &str,
    ) -> Result<Vec<TraceDetailRow>, OxyError> {
        unimplemented!()
    }
    async fn store_metric_usages(&self, _: Vec<MetricUsageRecord>) -> Result<(), OxyError> {
        unimplemented!()
    }
    async fn get_metrics_analytics(
        &self,
        _: &WorkspaceScope,
        _: u32,
    ) -> Result<MetricAnalyticsData, OxyError> {
        unimplemented!()
    }
    async fn get_metrics_list(
        &self,
        _: &WorkspaceScope,
        _: u32,
        _: usize,
        _: usize,
    ) -> Result<MetricsListData, OxyError> {
        unimplemented!()
    }
    async fn get_metric_detail(
        &self,
        _: &WorkspaceScope,
        _: &str,
        _: u32,
    ) -> Result<MetricDetailData, OxyError> {
        unimplemented!()
    }
    async fn get_execution_summary(
        &self,
        _: &WorkspaceScope,
        _: u32,
    ) -> Result<ExecutionSummaryData, OxyError> {
        unimplemented!()
    }
    async fn get_execution_time_series(
        &self,
        _: &WorkspaceScope,
        _: u32,
    ) -> Result<Vec<ExecutionTimeBucketData>, OxyError> {
        unimplemented!()
    }
    async fn get_execution_agent_stats(
        &self,
        _: &WorkspaceScope,
        _: u32,
        _: usize,
    ) -> Result<Vec<AgentExecutionStatsData>, OxyError> {
        unimplemented!()
    }
    #[allow(clippy::too_many_arguments)]
    async fn get_execution_list(
        &self,
        _: &WorkspaceScope,
        _: u32,
        _: usize,
        _: usize,
        _: Option<&str>,
        _: Option<bool>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<ExecutionListData, OxyError> {
        unimplemented!()
    }
    async fn insert_spans(&self, _: Vec<SpanRecord>) -> Result<(), OxyError> {
        unimplemented!()
    }
    async fn shutdown(&self) {}

    // The three reads this module makes (the per-app one is the test oracle).
    async fn get_app_availability(
        &self,
        _org_id: &str,
        app_id: &str,
        windows_minutes: &[u32],
    ) -> Result<Vec<AppAvailabilityWindow>, OxyError> {
        assert_eq!(windows_minutes, ALERT_WINDOWS_MINUTES);
        Ok(self.windows_of(app_id))
    }
    async fn get_fleet_availability(
        &self,
        apps: &[(String, String)],
        windows_minutes: &[u32],
    ) -> Result<HashMap<String, Vec<AppAvailabilityWindow>>, OxyError> {
        assert_eq!(windows_minutes, ALERT_WINDOWS_MINUTES);
        self.fleet_reads.fetch_add(1, Ordering::SeqCst);
        Ok(apps
            .iter()
            .map(|(_, app)| (app.clone(), self.windows_of(app)))
            .collect())
    }
    async fn get_fleet_build_sessions(
        &self,
        apps: &[(String, String)],
        _window_minutes: u32,
    ) -> Result<HashMap<String, Vec<AppBuildSessions>>, OxyError> {
        self.session_reads.fetch_add(1, Ordering::SeqCst);
        let sessions = self.sessions.lock().unwrap();
        Ok(apps
            .iter()
            .filter_map(|(_, app)| sessions.get(app).map(|s| (app.clone(), s.clone())))
            .collect())
    }
}

/// The one fake, installed as the process-global store on first use.
fn store() -> &'static Arc<FakeStore> {
    static STORE: OnceLock<Arc<FakeStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let store = Arc::new(FakeStore::default());
        oxy_observability::global::set_global(store.clone());
        store
    })
}

struct Seeded {
    app: AppRef,
    live: Option<Uuid>,
}

/// A published app with the given windows (5, 30, 60, 120, 360, 1440 minutes)
/// and session counts per build.
fn seed(
    store: &FakeStore,
    windows: Option<[(u64, u64); 6]>,
    sessions: &[(Uuid, u64)],
    live: Option<Uuid>,
) -> Seeded {
    let app = AppRef {
        id: Uuid::new_v4(),
        org_id: Uuid::new_v4(),
        published: true,
    };
    if let Some(w) = windows {
        let windows = ALERT_WINDOWS_MINUTES
            .iter()
            .zip(w)
            .map(|(m, (total, failed))| win(*m, total, failed))
            .collect();
        store
            .windows
            .lock()
            .unwrap()
            .insert(app.id.to_string(), windows);
    }
    store.sessions.lock().unwrap().insert(
        app.id.to_string(),
        sessions
            .iter()
            .map(|(build, n)| AppBuildSessions {
                build_id: build.to_string(),
                sessions: *n,
            })
            .collect(),
    );
    Seeded { app, live }
}

/// Production pointers, as the `app_environments` read would answer.
fn pointers(apps: &[&Seeded]) -> Option<HashMap<Uuid, Option<Uuid>>> {
    Some(apps.iter().map(|s| (s.app.id, s.live)).collect())
}

#[tokio::test]
async fn a_list_rows_availability_equals_a_per_app_read_of_its_windows() {
    let store = store();
    let live = Uuid::new_v4();
    let older = Uuid::new_v4();
    // Failing hard in the last hour and the last five minutes: pages.
    let burning = seed(
        store,
        Some([
            (30, 15),
            (60, 30),
            (100, 50),
            (150, 70),
            (300, 100),
            (500, 120),
        ]),
        &[(live, 3), (older, 2)],
        Some(live),
    );
    // Plenty of traffic, nothing failing.
    let healthy = seed(
        store,
        Some([(25, 0), (60, 0), (120, 0), (240, 0), (700, 0), (900, 1)]),
        &[(live, 9)],
        Some(live),
    );
    // Served nothing at all.
    let quiet = seed(store, None, &[], Some(live));
    let all = [&burning, &healthy, &quiet];
    let apps: Vec<AppRef> = all.iter().map(|s| s.app).collect();

    let rows = rows_for(&apps, |_| async { pointers(&all) }).await;

    for seeded in all {
        let listed = rows[&seeded.app.id]
            .1
            .clone()
            .expect("a measured app carries it");
        let windows = store
            .get_app_availability(
                &seeded.app.org_id.to_string(),
                &seeded.app.id.to_string(),
                ALERT_WINDOWS_MINUTES,
            )
            .await
            .expect("the per-app read");
        let standalone = AvailabilityReading::from_windows(&windows, listed.open_builds.clone());
        assert_eq!(listed, standalone);
        // The wire bytes too: skipped fields skip in both.
        assert_eq!(
            serde_json::to_value(&listed).unwrap(),
            serde_json::to_value(&standalone).unwrap()
        );
    }

    let verdict = |s: &Seeded| rows[&s.app.id].1.as_ref().unwrap().verdict;
    assert_eq!(verdict(&burning), "burning");
    assert_eq!(verdict(&healthy), "healthy");
    // No traffic is "no opinion", never "healthy".
    assert_eq!(verdict(&quiet), "no_opinion");
    let burn = rows[&burning.app.id].1.as_ref().unwrap();
    assert_eq!(
        (burn.severity, burn.long_window_minutes),
        (Some("page"), Some(60))
    );
    assert_eq!(burn.open_builds.status, "some_older");
}

#[tokio::test]
async fn a_second_page_inside_the_ttl_reads_nothing() {
    let store = store();
    let live = Uuid::new_v4();
    let a = seed(store, Some([(30, 0); 6]), &[(live, 1)], Some(live));
    let apps = [a.app];

    let first = rows_for(&apps, |_| async { pointers(&[&a]) }).await;
    let (fleet, sessions) = (
        store.fleet_reads.load(Ordering::SeqCst),
        store.session_reads.load(Ordering::SeqCst),
    );
    assert_eq!((fleet, sessions), (1, 1));

    let asked = AtomicBool::new(false);
    let second = rows_for(&apps, |_| async {
        asked.store(true, Ordering::SeqCst);
        pointers(&[&a])
    })
    .await;

    assert_eq!(second[&a.app.id], first[&a.app.id]);
    assert_eq!(store.fleet_reads.load(Ordering::SeqCst), fleet);
    assert_eq!(store.session_reads.load(Ordering::SeqCst), sessions);
    assert!(
        !asked.load(Ordering::SeqCst),
        "pointers are not re-read either"
    );
}

#[tokio::test]
async fn a_draft_carries_no_availability_and_is_not_sent_to_the_store() {
    let store = store();
    let draft = AppRef {
        id: Uuid::new_v4(),
        org_id: Uuid::new_v4(),
        published: false,
    };

    let rows = rows_for(&[draft], |_| async {
        panic!("a draft has no live build to read")
    })
    .await;

    assert_eq!(rows[&draft.id], (ListVerdict::Draft, None));
    assert_eq!(store.fleet_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn open_builds_that_could_not_be_read_are_not_measured_and_not_remembered() {
    let store = store();
    let live = Uuid::new_v4();
    let a = seed(store, Some([(30, 0); 6]), &[(live, 4)], Some(live));

    let failed = rows_for(&[a.app], |_| async { None }).await;
    let line = &failed[&a.app.id].1.as_ref().unwrap().open_builds;
    assert_eq!(line.status, "not_measured");

    // The next page reads them again and gets the real line.
    let next = rows_for(&[a.app], |_| async { pointers(&[&a]) }).await;
    assert_eq!(
        next[&a.app.id].1.as_ref().unwrap().open_builds.status,
        "all_live"
    );
}
