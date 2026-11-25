use std::sync::Mutex;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

use super::*;
use oxy_app_core::seams::{CUSTOM_APP_FLEET_JOBS, require_fleet_jobs};

/// What the fake jobs below saw: their name, per start.
static STARTED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record(name: &'static str) {
    STARTED.lock().unwrap_or_else(|p| p.into_inner()).push(name);
}

fn custom_app_jobs() -> Vec<FleetJobSeam> {
    vec![
        FleetJobSeam {
            name: "custom_app_asset_lifecycle_verify",
            start: |_| record("custom_app_asset_lifecycle_verify"),
        },
        FleetJobSeam {
            name: "custom_app_storage_sweeper",
            start: |_| record("custom_app_storage_sweeper"),
        },
        FleetJobSeam {
            name: "custom_app_usage_backfill_sweep",
            start: |_| record("custom_app_usage_backfill_sweep"),
        },
        FleetJobSeam {
            name: "custom_app_event_relay_sweep",
            start: |_| record("custom_app_event_relay_sweep"),
        },
        FleetJobSeam {
            name: "custom_app_event_retention",
            start: |_| record("custom_app_event_retention"),
        },
        FleetJobSeam {
            name: "custom_app_table_watch_sweep",
            start: |_| record("custom_app_table_watch_sweep"),
        },
        FleetJobSeam {
            name: "oltp_changes_relay_sweep",
            start: |_| record("oltp_changes_relay_sweep"),
        },
        FleetJobSeam {
            name: "oltp_changes_weekly_sweep",
            start: |_| record("oltp_changes_weekly_sweep"),
        },
    ]
}

/// The worker's half of the bug: a process that runs singleton work starts
/// every job it is handed, plus the audit anchor, and a serve replica starts
/// none. One test so the shared `STARTED` list is not raced.
#[tokio::test]
async fn a_singleton_process_starts_every_fleet_job_and_a_serve_replica_none() {
    // The audit anchor's own start reads this; unset, it logs and returns.
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var("OXY_AUDIT_ANCHOR_S3_BUCKET") };
    let db = DatabaseConnection::default();
    let shutdown = CancellationToken::new();
    let deps = FleetJobDeps {
        db: &db,
        shutdown: &shutdown,
    };
    let jobs = custom_app_jobs();

    assert!(start_fleet_jobs(&jobs, &deps, false).is_empty());
    assert!(
        STARTED.lock().unwrap().is_empty(),
        "a serve replica started a fleet job"
    );

    let started = start_fleet_jobs(&jobs, &deps, true);
    assert_eq!(
        started,
        [
            "audit_chain_anchor",
            "custom_app_asset_lifecycle_verify",
            "custom_app_storage_sweeper",
            "custom_app_usage_backfill_sweep",
            "custom_app_event_relay_sweep",
            "custom_app_event_retention",
            "custom_app_table_watch_sweep",
            "oltp_changes_relay_sweep",
            "oltp_changes_weekly_sweep",
        ]
    );
    assert_eq!(*STARTED.lock().unwrap(), CUSTOM_APP_FLEET_JOBS);
}

#[test]
fn the_custom_app_fleet_jobs_are_required() {
    assert_eq!(require_fleet_jobs(&custom_app_jobs()), Ok(()));

    let mut jobs = custom_app_jobs();
    jobs.retain(|j| j.name != "custom_app_storage_sweeper");
    let err = require_fleet_jobs(&jobs).unwrap_err();
    assert!(err.contains("custom_app_storage_sweeper"), "{err}");

    let err = require_fleet_jobs(&[]).unwrap_err();
    assert!(err.contains("custom_app_asset_lifecycle_verify"), "{err}");
}

#[test]
fn a_fleet_job_registered_twice_is_refused() {
    let mut jobs = custom_app_jobs();
    jobs.push(jobs[0]);
    let err = require_fleet_jobs(&jobs).unwrap_err();
    assert!(err.contains("registered twice"), "{err}");
}
