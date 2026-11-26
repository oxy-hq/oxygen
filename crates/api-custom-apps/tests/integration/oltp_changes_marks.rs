//! The change stream relay's platform half (`oltp_changes::relay::marks` and
//! `holds`), on the control plane's database. Time is steered with timestamps
//! written in SQL and zero or small margins, never with sleeps. Each test has
//! its own org, so a concurrent test's rows are never touched, with one
//! exception: `mark_all` is the weekly sweep and nudges every row. Tests run
//! in separate processes, so a Postgres advisory lock guards it: the sweep's
//! test holds it exclusively, and every test asserting a mark is not due holds
//! it shared ([`guard`]), so the sweep never lands inside one of those.

use std::time::Duration;

use chrono::{DateTime, Utc};
use oxy_api_custom_apps::oltp_changes::relay::holds::{self, HoldKey};
use oxy_api_custom_apps::oltp_changes::relay::marks::{
    acquire, drained, due, failed, mark_all, nudge, release, renew, succeeded, wake,
};
use oxy_api_custom_apps::oltp_changes::relay::sweep::{sweep_once, weekly_once};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction,
    EntityTrait, Statement, TransactionTrait,
};
use serde_json::json;
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, publish_app, seeded_tenant, throwaway_org};
use crate::custom_app_functions_shape_zoo::{compile, write_config};

pub(crate) const ENV: &str = "production";
pub(crate) const MARGIN: Duration = Duration::from_secs(30);
const NOW: Duration = Duration::ZERO;
/// The advisory lock key shared by this file's tests.
const SWEEP_LOCK: i64 = 0x6f6c_7470_7377_6570;

/// Hold the sweep lock, shared or exclusive, until the returned transaction is
/// dropped at the end of the test. Every test of any file that calls
/// `mark_all` or `weekly_once` holds it exclusively, and every test asserting
/// a mark is not due holds it shared.
pub(crate) async fn guard(db: &DatabaseConnection, exclusive: bool) -> DatabaseTransaction {
    let tx = db.begin().await.expect("begin");
    let lock = if exclusive {
        "pg_advisory_xact_lock"
    } else {
        "pg_advisory_xact_lock_shared"
    };
    tx.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("SELECT {lock}($1)"),
        [SWEEP_LOCK.into()],
    ))
    .await
    .expect("lock");
    tx
}

pub(crate) async fn org() -> Tenant {
    throwaway_org(&seeded_tenant().await).await
}

pub(crate) async fn is_due(db: &DatabaseConnection, org_id: Uuid, margin: Duration) -> bool {
    due(db, margin, NOW)
        .await
        .expect("due")
        .iter()
        .any(|d| d.org_id == org_id && d.environment == ENV)
}

pub(crate) async fn exec(db: &DatabaseConnection, sql: &str, org_id: Uuid) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [org_id.into(), ENV.into()],
    ))
    .await
    .expect("exec");
}

pub(crate) async fn instant(
    db: &DatabaseConnection,
    org_id: Uuid,
    column: &str,
) -> Option<DateTime<Utc>> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT {column} AS v FROM oltp_change_relays WHERE org_id = $1 AND environment = $2"
        ),
        [org_id.into(), ENV.into()],
    ))
    .await
    .expect("query")
    .expect("the mark")
    .try_get("", "v")
    .expect("instant")
}

/// Nudge, then move the nudge a minute into the past, so every drain instant
/// a test picks around it is in the past too and only the margin and `every`
/// clauses decide. Returns the backdated `nudged_at`.
pub(crate) async fn nudged_a_minute_ago(db: &DatabaseConnection, org_id: Uuid) -> DateTime<Utc> {
    nudge(db, org_id, ENV).await.expect("nudge");
    exec(
        db,
        "UPDATE oltp_change_relays SET nudged_at = now() - interval '60 seconds', \
                pending_since = now() - interval '60 seconds' \
          WHERE org_id = $1 AND environment = $2",
        org_id,
    )
    .await;
    instant(db, org_id, "nudged_at").await.expect("nudged_at")
}

fn secs(n: i64) -> chrono::Duration {
    chrono::Duration::seconds(n)
}

#[tokio::test]
async fn a_nudge_makes_a_mark_due() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    assert!(!is_due(&t.db, t.org_id, MARGIN).await);
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    assert!(is_due(&t.db, t.org_id, MARGIN).await);
}

#[tokio::test]
async fn a_drain_started_after_the_margin_settles_the_mark() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    // Both drains are in the past: only the margin tells them apart.
    drained(&t.db, t.org_id, ENV, nudged + secs(29), MARGIN)
        .await
        .expect("drained");
    assert!(is_due(&t.db, t.org_id, MARGIN).await, "29s is inside 30s");
    drained(&t.db, t.org_id, ENV, nudged + secs(31), MARGIN)
        .await
        .expect("drained");
    assert!(!is_due(&t.db, t.org_id, MARGIN).await, "31s is past 30s");
    assert_eq!(instant(&t.db, t.org_id, "pending_since").await, None);
}

#[tokio::test]
async fn a_drain_inside_the_margin_keeps_the_mark_due() {
    let t = org().await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    let inside = nudged + secs(10);
    // The drain began before the nudge, and again inside the margin after it.
    for started in [nudged - secs(1), inside] {
        drained(&t.db, t.org_id, ENV, started, MARGIN)
            .await
            .expect("drained");
        assert!(is_due(&t.db, t.org_id, MARGIN).await);
    }
    let waiting = due(&t.db, MARGIN, NOW).await.expect("due");
    let mine = waiting.iter().find(|d| d.org_id == t.org_id).expect("due");
    assert_eq!(mine.pending_since, inside);
}

#[tokio::test]
async fn a_drain_within_every_is_not_due_until_every_has_passed() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    // The drain began 20s ago, inside the margin of a nudge 10s ago.
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET nudged_at = now() - interval '10 seconds', \
                drain_started_at = now() - interval '20 seconds' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    let due_with = async |every: u64| {
        due(&t.db, MARGIN, Duration::from_secs(every))
            .await
            .expect("due")
            .iter()
            .any(|d| d.org_id == t.org_id)
    };
    assert!(!due_with(60).await, "the drain began within `every`");
    assert!(
        due_with(10).await,
        "`every` has passed since the drain began"
    );
}

/// The production mark's due entry, if it is due.
async fn due_entry(db: &DatabaseConnection, org_id: Uuid) -> Option<bool> {
    due(db, MARGIN, NOW)
        .await
        .expect("due")
        .into_iter()
        .find(|d| d.org_id == org_id && d.environment == ENV)
        .map(|d| d.waiting)
}

/// A wake leaves a drained mark drained and counts no failure: the mark is
/// due once its `wake_at` passes, with no change waiting.
#[tokio::test]
async fn a_wake_makes_a_drained_mark_due_without_a_failure() {
    let t = org().await;
    let _sweep = guard(&t.db, false).await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    drained(&t.db, t.org_id, ENV, nudged + secs(31), MARGIN)
        .await
        .expect("drained");
    wake(&t.db, t.org_id, ENV, Duration::from_secs(3600))
        .await
        .expect("wake");
    assert_eq!(due_entry(&t.db, t.org_id).await, None, "not before wake_at");
    assert!(instant(&t.db, t.org_id, "wake_at").await.is_some());
    assert!(instant(&t.db, t.org_id, "drain_started_at").await.is_some());
    assert_eq!(instant(&t.db, t.org_id, "failed_at").await, None);
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET wake_at = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    assert_eq!(
        due_entry(&t.db, t.org_id).await,
        Some(false),
        "due at wake_at, nothing waiting"
    );
}

/// A wake changes nothing of PR 2's rule: a drain that began inside the
/// margin after a nudge, as a debounced write's, keeps the mark due, waiting.
#[tokio::test]
async fn a_debounced_write_keeps_a_woken_mark_due() {
    let t = org().await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    drained(&t.db, t.org_id, ENV, nudged + secs(10), MARGIN)
        .await
        .expect("drained");
    wake(&t.db, t.org_id, ENV, Duration::from_secs(3600))
        .await
        .expect("wake");
    assert_eq!(due_entry(&t.db, t.org_id).await, Some(true));
}

/// A failed pass keeps the wake it ran for, no sooner than its own backoff's
/// end, so the baseline is still retried after the failure; a pass with no
/// failure clears it.
#[tokio::test]
async fn a_failed_pass_keeps_its_wake_and_a_passing_one_clears_it() {
    let t = org().await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    wake(&t.db, t.org_id, ENV, Duration::from_secs(60))
        .await
        .expect("wake");
    let hour = Duration::from_secs(3600);
    failed(&t.db, t.org_id, ENV, hour, hour)
        .await
        .expect("failed");
    let woken = instant(&t.db, t.org_id, "wake_at").await.expect("kept");
    let retry = instant(&t.db, t.org_id, "retry_after")
        .await
        .expect("backed off");
    assert!(
        woken >= retry,
        "no sooner than the backoff: {woken} {retry}"
    );
    succeeded(&t.db, t.org_id, ENV).await.expect("succeeded");
    assert_eq!(instant(&t.db, t.org_id, "wake_at").await, None);
}

#[tokio::test]
async fn a_second_nudge_keeps_the_first_pending_since() {
    let t = org().await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET pending_since = now() - interval '100 seconds' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    let first = instant(&t.db, t.org_id, "pending_since").await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    assert_eq!(instant(&t.db, t.org_id, "pending_since").await, first);
}

#[tokio::test]
async fn the_weekly_sweep_makes_a_drained_mark_due_again() {
    let t = org().await;
    let _sweep = guard(&t.db, true).await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    drained(&t.db, t.org_id, ENV, nudged + secs(31), MARGIN)
        .await
        .expect("drained");
    assert!(!is_due(&t.db, t.org_id, MARGIN).await);
    assert!(mark_all(&t.db).await.expect("mark_all") >= 1);
    // The sweep's nudge is now, so the drain (31s into a minute-old nudge)
    // no longer covers it.
    assert!(is_due(&t.db, t.org_id, MARGIN).await);
}

#[tokio::test]
async fn a_second_lease_is_refused_until_the_first_expires() {
    let t = org().await;
    let ttl = Duration::from_secs(120);
    let first = acquire(&t.db, t.org_id, ENV, "a", ttl)
        .await
        .expect("acquire")
        .expect("the first lease");
    assert_eq!(first.epoch, 1);
    assert!(
        acquire(&t.db, t.org_id, ENV, "b", ttl)
            .await
            .expect("acquire")
            .is_none()
    );
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET lease_expires_at = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    let second = acquire(&t.db, t.org_id, ENV, "b", ttl)
        .await
        .expect("acquire")
        .expect("the lease once the first expired");
    assert_eq!((second.holder.as_str(), second.epoch), ("b", 2));
}

#[tokio::test]
async fn a_renewal_by_a_stale_holder_fails() {
    let t = org().await;
    let ttl = Duration::from_secs(120);
    let first = acquire(&t.db, t.org_id, ENV, "a", ttl)
        .await
        .expect("acquire")
        .expect("lease");
    assert!(renew(&t.db, &first, ttl).await.expect("renew"));
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET lease_expires_at = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    assert!(
        !renew(&t.db, &first, ttl).await.expect("renew"),
        "an expired lease is not renewed"
    );
    let second = acquire(&t.db, t.org_id, ENV, "b", ttl)
        .await
        .expect("acquire")
        .expect("lease");
    assert!(
        !renew(&t.db, &first, ttl).await.expect("renew"),
        "the first holder lost it to the second"
    );
    assert!(renew(&t.db, &second, ttl).await.expect("renew"));
}

#[tokio::test]
async fn a_hold_is_kept_with_its_first_reason() {
    let t = org().await;
    let key = |writer, table| HoldKey {
        org_id: t.org_id,
        environment: ENV,
        writer,
        table,
    };
    let orders = key("w", Some("orders"));
    let first = holds::hold(&t.db, orders, "renamed column")
        .await
        .expect("hold");
    let again = holds::hold(&t.db, orders, "another reason")
        .await
        .expect("hold");
    assert!((first, again) == (true, false));
    holds::hold(&t.db, key("v", None), "whole writer")
        .await
        .expect("hold");
    let read = holds::read(&t.db, t.org_id, ENV).await.expect("read");
    assert_eq!(read.tables("w"), vec!["orders".to_string()]);
    assert!(!read.writer("w") && read.writer("v"));
    let reason: String =
        t.db.query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT reason FROM oltp_change_holds \
              WHERE org_id = $1 AND environment = $2 AND writer = 'w'",
            [t.org_id.into(), ENV.into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get("", "reason")
        .expect("reason");
    assert_eq!(reason, "renamed column");
}

#[tokio::test]
async fn a_released_lease_can_be_taken_and_the_epoch_keeps_counting() {
    let t = org().await;
    let ttl = Duration::from_secs(120);
    let first = acquire(&t.db, t.org_id, ENV, "a", ttl)
        .await
        .expect("acquire")
        .expect("lease");
    release(&t.db, &first).await.expect("release");
    let second = acquire(&t.db, t.org_id, ENV, "b", ttl)
        .await
        .expect("acquire")
        .expect("a released lease is free at once");
    assert_eq!(second.epoch, first.epoch + 1);
    // Releasing with the old lease touches nothing of the new holder's.
    release(&t.db, &first).await.expect("release");
    assert!(renew(&t.db, &second, ttl).await.expect("renew"));
}

#[tokio::test]
async fn a_renewal_with_an_older_epoch_fails_though_the_holder_matches() {
    let t = org().await;
    let ttl = Duration::from_secs(120);
    let old = acquire(&t.db, t.org_id, ENV, "a", ttl)
        .await
        .expect("acquire")
        .expect("lease");
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET lease_expires_at = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    let new = acquire(&t.db, t.org_id, ENV, "a", ttl)
        .await
        .expect("acquire")
        .expect("lease");
    assert_eq!((old.holder.as_str(), new.epoch), ("a", old.epoch + 1));
    assert!(!renew(&t.db, &old, ttl).await.expect("renew"));
    assert!(renew(&t.db, &new, ttl).await.expect("renew"));
}

/// Turn `oltp_changes` on for `t`'s org, keeping the orgs other tests of this
/// shared database turned on, and load the flag cache from the table. The
/// override is one row for every org, so a concurrent test's write may land
/// between this one's read and write: retry until the cache says it is on.
pub(crate) async fn enable_oltp_changes(t: &Tenant) {
    for _ in 0..20 {
        let existing =
            entity::feature_flag::Entity::find_by_id(oxy_feature_flags::OLTP_CHANGES.to_string())
                .one(&t.db)
                .await
                .expect("read the flag");
        let mut orgs = existing
            .and_then(|f| f.targeting)
            .and_then(|targeting| targeting.get("orgs").and_then(|o| o.as_object().cloned()))
            .unwrap_or_default();
        orgs.insert(t.org_id.to_string(), json!("on"));
        entity::feature_flag::Entity::insert(entity::feature_flag::ActiveModel {
            key: Set(oxy_feature_flags::OLTP_CHANGES.to_string()),
            // The registry default; the kill switch is in the document.
            enabled: Set(false),
            updated_at: Set(chrono::Utc::now().fixed_offset()),
            targeting: Set(Some(json!({
                "enabled": true,
                "orgs": orgs,
                "fallthrough": { "type": "variant", "variant": "off" },
            }))),
        })
        .on_conflict(
            OnConflict::column(entity::feature_flag::Column::Key)
                .update_columns([
                    entity::feature_flag::Column::Enabled,
                    entity::feature_flag::Column::UpdatedAt,
                    entity::feature_flag::Column::Targeting,
                ])
                .to_owned(),
        )
        .exec(&t.db)
        .await
        .expect("write the oltp_changes override");
        oxy_feature_flags::cache::load_once()
            .await
            .expect("load the flag cache");
        if oxy_api_custom_apps::oltp_changes::enabled_for(t.org_id) {
            return;
        }
    }
    panic!("oltp_changes would not turn on for the org");
}

/// An org with a published app, which a relay's run is queued under.
pub(crate) async fn org_with_an_app() -> Tenant {
    let t = org().await;
    let root = write_config("  - name: duck\n    type: duckdb\n    path: changes.duckdb\n");
    let workspace = compile(&t, root.path()).await;
    let slug = format!("oltp-sweep-{}", &Uuid::new_v4().simple().to_string()[..8]);
    publish_app(&t, &slug, workspace, &[]).await;
    t
}

/// The relay tasks waiting to run for `org_id`'s production store.
pub(crate) async fn queued_relays(db: &DatabaseConnection, org_id: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM agentic_task_queue \
          WHERE task_id LIKE $1 AND queue_status = 'queued'",
        [format!("oltp_changes_relay:{org_id}:{ENV}:%").into()],
    ))
    .await
    .expect("query")
    .expect("a count")
    .try_get("", "n")
    .expect("n")
}

#[tokio::test]
async fn the_sweep_queues_one_relay_per_due_mark() {
    let t = org_with_an_app().await;
    let _sweep = guard(&t.db, false).await;
    enable_oltp_changes(&t).await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");

    let first = sweep_once(&t.db).await.expect("sweep");
    assert!(first.queued >= 1, "the due mark is queued: {first:?}");
    assert_eq!(queued_relays(&t.db, t.org_id).await, 1);
    // The mark is still due (no pass ran), and the waiting task is enough.
    sweep_once(&t.db).await.expect("sweep again");
    assert_eq!(queued_relays(&t.db, t.org_id).await, 1);
}

#[tokio::test]
async fn the_sweep_passes_over_orgs_without_the_flag() {
    let t = org_with_an_app().await;
    let _sweep = guard(&t.db, false).await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    assert!(is_due(&t.db, t.org_id, MARGIN).await);

    let report = sweep_once(&t.db).await.expect("sweep");
    assert!(!report.stale.contains(&t.org_id));
    assert_eq!(queued_relays(&t.db, t.org_id).await, 0);
    assert!(
        is_due(&t.db, t.org_id, MARGIN).await,
        "the mark is untouched"
    );
}

#[tokio::test]
async fn the_weekly_sweep_makes_the_next_sweep_queue_a_drained_mark() {
    let t = org_with_an_app().await;
    let _sweep = guard(&t.db, true).await;
    enable_oltp_changes(&t).await;
    let nudged = nudged_a_minute_ago(&t.db, t.org_id).await;
    drained(&t.db, t.org_id, ENV, nudged + secs(31), MARGIN)
        .await
        .expect("drained");
    sweep_once(&t.db).await.expect("sweep");
    assert_eq!(queued_relays(&t.db, t.org_id).await, 0, "nothing is due");

    assert!(weekly_once(&t.db).await.expect("weekly") >= 1);
    sweep_once(&t.db).await.expect("sweep");
    assert_eq!(queued_relays(&t.db, t.org_id).await, 1);
}

#[tokio::test]
async fn a_mark_waiting_past_the_stale_bound_is_reported() {
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
    let report = sweep_once(&t.db).await.expect("sweep");
    assert!(report.stale.contains(&t.org_id), "{report:?}");
}

/// A mark due only to wake a baseline has no change waiting: however old its
/// last nudge, the sweep queues it and never reports it stale.
#[tokio::test]
async fn a_mark_due_only_to_wake_is_not_reported_stale() {
    let t = org_with_an_app().await;
    let _sweep = guard(&t.db, false).await;
    enable_oltp_changes(&t).await;
    nudge(&t.db, t.org_id, ENV).await.expect("nudge");
    exec(
        &t.db,
        "UPDATE oltp_change_relays SET nudged_at = now() - interval '10 minutes', \
                pending_since = NULL, drain_started_at = now() - interval '9 minutes', \
                wake_at = now() - interval '1 second' \
          WHERE org_id = $1 AND environment = $2",
        t.org_id,
    )
    .await;
    assert_eq!(due_entry(&t.db, t.org_id).await, Some(false));
    let report = sweep_once(&t.db).await.expect("sweep");
    assert!(!report.stale.contains(&t.org_id), "{report:?}");
}
