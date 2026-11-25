//! The change stream relay's backoff and what its sweeps reach, on the
//! control plane's database: a failed pass backs its mark off, doubling to a
//! cap; a nudge after the failure makes it due anyway; a pass with no failed
//! writer ends it; a backed-off mark alarms stale once per backoff window;
//! and the weekly sweep marks a flagged org's store that was never nudged.
//!
//! Every test asserting a mark is not due, or calling the weekly sweep, holds
//! `oltp_changes_marks`'s sweep lock ([`guard`]), as its tests do.

use oxy_api_custom_apps::oltp_changes::relay::marks::{failed, nudge, succeeded};
use oxy_api_custom_apps::oltp_changes::relay::sweep::{sweep_once, weekly_once};
use oxy_api_custom_apps::oltp_changes::relay::{BACKOFF_BASE, BACKOFF_CAP};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use crate::oltp_changes_fixture::with_app;
use crate::oltp_changes_marks::{
    ENV, MARGIN, enable_oltp_changes, exec, guard, instant, is_due, nudged_a_minute_ago, org,
    org_with_an_app,
};

async fn fail(db: &DatabaseConnection, org_id: Uuid) {
    failed(db, org_id, ENV, BACKOFF_BASE, BACKOFF_CAP)
        .await
        .expect("record the failure");
}

/// The mark's `failures`, and its backoff in whole seconds.
async fn backoff(db: &DatabaseConnection, org_id: Uuid) -> (i32, Option<i64>) {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT failures, \
                    round(EXTRACT(EPOCH FROM retry_after - failed_at))::bigint AS backoff \
               FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
            [org_id.into(), ENV.into()],
        ))
        .await
        .expect("read the mark")
        .expect("the mark");
    (
        row.try_get("", "failures").expect("failures"),
        row.try_get("", "backoff").expect("backoff"),
    )
}

#[tokio::test]
async fn a_failed_pass_backs_its_mark_off_until_retry_after() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    nudged_a_minute_ago(&t.db, t.org_id).await;
    assert!(is_due(&t.db, t.org_id, MARGIN).await);
    fail(&t.db, t.org_id).await;
    assert!(!is_due(&t.db, t.org_id, MARGIN).await, "backed off");
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET retry_after = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    assert!(is_due(&t.db, t.org_id, MARGIN).await, "retry_after passed");
}

#[tokio::test]
async fn a_nudge_after_the_failure_is_due_despite_the_backoff() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    nudged_a_minute_ago(&t.db, t.org_id).await;
    fail(&t.db, t.org_id).await;
    assert!(!is_due(&t.db, t.org_id, MARGIN).await, "backed off");
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    assert!(is_due(&t.db, t.org_id, MARGIN).await, "a fresh write");
}

#[tokio::test]
async fn a_pass_with_no_failed_writer_ends_the_backoff() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    nudged_a_minute_ago(&t.db, t.org_id).await;
    fail(&t.db, t.org_id).await;
    fail(&t.db, t.org_id).await;
    assert_eq!(backoff(&t.db, t.org_id).await, (2, Some(10)));
    succeeded(&t.db, t.org_id, ENV).await.expect("succeeded");
    assert_eq!(backoff(&t.db, t.org_id).await, (0, None));
    assert_eq!(instant(&t.db, t.org_id, "failed_at").await, None);
    assert!(is_due(&t.db, t.org_id, MARGIN).await);
}

/// 5 s after the first failure, doubling for each after it, to 30 min.
#[tokio::test]
async fn the_backoff_doubles_to_its_cap() {
    let t = org().await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    let mut seen = Vec::new();
    for _ in 0..12 {
        fail(&t.db, t.org_id).await;
        seen.push(backoff(&t.db, t.org_id).await.1.expect("a backoff"));
    }
    let want = [5, 10, 20, 40, 80, 160, 320, 640, 1280, 1800, 1800, 1800];
    assert_eq!(seen, want);
    assert_eq!(backoff(&t.db, t.org_id).await.0, 12);
}

/// A nudge makes a backed-off mark due at every sweep; its stale alarm is
/// logged once in each backoff window, then again after the next failure.
#[tokio::test]
async fn a_backed_off_mark_alarms_stale_once_per_window() {
    let t = org_with_an_app().await;
    let _sweep = guard(&t.db, false).await;
    enable_oltp_changes(&t).await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET pending_since = now() - interval '10 minutes' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    let alarmed = async || {
        let report = sweep_once(&t.db).await.expect("sweep");
        report.stale.contains(&t.org_id)
    };
    for window in 1..=2 {
        fail(&t.db, t.org_id).await;
        // A window long enough that no slow sweep outlives it.
        exec(
            &t.db,
            "UPDATE oltp_change_relays SET retry_after = now() + interval '1 hour' \
              WHERE org_id = $1 AND environment = $2",
            t.org_id,
        )
        .await;
        nudge(&t.db, t.org_id, ENV).await.expect("nudge");
        assert!(
            alarmed().await,
            "the window's first sweep alarms ({window})"
        );
        assert!(!alarmed().await, "its next sweep does not ({window})");
    }
}

async fn has_mark(db: &DatabaseConnection, org_id: Uuid) -> bool {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 AS one FROM oltp_change_relays WHERE org_id = $1 AND environment = $2",
        [org_id.into(), ENV.into()],
    ))
    .await
    .expect("read the mark")
    .is_some()
}

/// A flagged org's active store that no nudge ever announced (one that
/// captured before its org was flagged, say) gets a due production mark from
/// the weekly sweep, so the 5 s sweep drains it.
#[tokio::test]
async fn the_weekly_sweep_marks_a_flagged_store_never_nudged() {
    with_app(|app| async move {
        let db = &app.t.db;
        let _sweep = guard(db, true).await;
        assert!(!has_mark(db, app.org()).await, "never nudged");
        weekly_once(db).await.expect("weekly");
        assert!(instant(db, app.org(), "nudged_at").await.is_some());
        assert!(is_due(db, app.org(), MARGIN).await);
    })
    .await;
}
