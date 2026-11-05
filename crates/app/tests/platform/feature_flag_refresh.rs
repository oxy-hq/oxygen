//! The feature-flag cache's periodic refresh — the property that makes a flag
//! flip fleet-wide. Without it a PATCH reaches only the instance it lands on;
//! nothing else in the repo pins that a DB change propagates.

use std::time::Duration;

use oxy_app::server::feature_flags::{cache, is_enabled, store};

use crate::common;

/// A flag flipped in the DB — as a PATCH on ANOTHER instance would leave it —
/// reaches this process's cache within a refresh interval.
///
/// Drives the loop at 100ms via `OXY_FEATURE_FLAG_REFRESH_MS`, so the 15s
/// production default does not make this a 15s test. nextest's process-per-test
/// isolation is what makes setting that env var and touching the global cache
/// statics sound.
#[tokio::test]
async fn a_db_change_propagates_to_the_cache_via_the_refresh() {
    // Safety: nextest runs each test in its own process, so this env write and
    // the global cache below are not shared with any sibling. Set BEFORE `init`,
    // which reads the interval once at spawn.
    unsafe {
        std::env::set_var("OXY_FEATURE_FLAG_REFRESH_MS", "100");
    }

    // A migrated per-test DB, with `OXY_DATABASE_URL` pointed at it — which is
    // the pool `cache::init` and the refresh open, AND the connection this `db`
    // handle uses, so a row written here is the row the refresh reads.
    let db = common::test_db().await;

    // Fallible now: a failed load is fatal for serve, so the test asserts it
    // succeeded rather than silently proceeding on an unloaded cache (which
    // would make the pre-condition below pass for the wrong reason).
    cache::init().await.expect("feature flag cache init");
    assert!(
        !is_enabled("billing"),
        "cache loaded and the DB has no billing row, so it reads the default (off)"
    );

    // Another instance flips it on (its PATCH commits this row + updates ITS
    // cache; ours only sees the row).
    store::upsert(&db, "billing", true)
        .await
        .expect("upsert billing=true");

    // Our cache still reads off until a refresh tick picks the row up.
    let mut flipped = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if is_enabled("billing") {
            flipped = true;
            break;
        }
    }
    assert!(
        flipped,
        "the refresh must propagate a DB change to this process — without it a \
         flag flip reaches only the instance the PATCH lands on"
    );
}

/// A row for a flag the registry no longer declares — `oltp`, removed when
/// per-org OLTP went always-on, still sits in staging and prod — is ignored at
/// load rather than failing it. A failed load is fatal for serve, so choking on
/// a leftover row would stop every instance from booting.
#[tokio::test]
async fn a_row_for_a_removed_flag_does_not_break_the_load() {
    let db = common::test_db().await;
    store::upsert(&db, "oltp", true)
        .await
        .expect("upsert the leftover oltp row");

    cache::init()
        .await
        .expect("a leftover row must not fail the load");
    assert!(
        !is_enabled("billing"),
        "the registered flag still reads its own value (default off)"
    );
    assert!(
        !is_enabled("oltp"),
        "an unregistered key reads false, whatever its row says"
    );
}

/// A failed load must be REPORTABLE — `cache::init` returns `Err`. That is the
/// property that lets `serve` `?` it and refuse to boot with an unknown billing
/// state; the regression it guards (init made infallible, so the paywall could
/// silently open) shipped one commit before it was caught. Revert the `?` or
/// widen the signature back and this fails.
#[tokio::test]
async fn a_failed_load_is_reported_so_serve_can_fail_fast() {
    // An invalid DSN, so the load fails deterministically and instantly. A
    // closed port would also fail, but "connection refused" is classified
    // transient and retried 8× with backoff (~seconds); a malformed URL fails
    // once on the same `establish_connection` path, proving the same property
    // without the wait. Process-per-test isolation makes this env write sound.
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", "not-a-postgres-url");
    }
    assert!(
        cache::init().await.is_err(),
        "a failed flag load must surface as Err — serve `?`s it rather than accept \
         requests with an unknown billing state"
    );
}

/// Before any init the cache is unloaded, and `is_enabled` returns the registry
/// default — billing OFF, i.e. the paywall SKIPPED. This is the exact fail-open
/// hazard that makes `cache::init` fallible: an un-inited process would let every
/// org past the paywall, so serve must not serve until the load succeeds. Named
/// for that reason, and it breaks loudly if someone flips `billing`'s default to
/// on to "fix" the unloaded read instead.
#[tokio::test]
async fn an_unloaded_cache_reads_billing_off_so_the_paywall_would_skip() {
    // No `cache::init` in this (isolated) process: INITIALIZED stays false, so
    // reads fall through to the registry default.
    assert!(
        !is_enabled("billing"),
        "unloaded cache must read billing OFF (registry default) — the fail-open \
         posture serve avoids by treating init as fatal"
    );
    assert!(
        oxy_app::server::api::billing::billing_disabled(),
        "and the gate the subscription guard consults reads disabled, so an \
         un-inited process would skip the paywall for every org"
    );
}

/// A flag with rules keeps its kill switch inside `targeting` and writes the
/// `enabled` column as the registry default — what a binary that predates
/// targeting (one revert back) reads. So a revert sees a targeted flag as
/// untouched, never as on for every org. This build reads the rules back.
#[tokio::test]
async fn a_targeted_flag_reads_as_its_default_to_a_binary_without_targeting() {
    use oxy_app::server::feature_flags::targeting::{Fallthrough, Targeting};
    use oxy_app::server::feature_flags::{FlagContext, is_enabled_for, registry};

    let db = common::test_db().await;
    let def = registry::get("billing").expect("billing is registered");
    let held_back = uuid::Uuid::new_v4();
    let targeting = Targeting {
        users: Default::default(),
        orgs: [(held_back, "off".to_string())].into(),
        fallthrough: Fallthrough::Variant {
            variant: "on".into(),
        },
        salt: None,
    };
    let row = store::write(&db, def, true, Some(&targeting))
        .await
        .expect("write targeted billing");
    assert_eq!(
        row.enabled, def.default_enabled,
        "the legacy column must read the default while rules are set"
    );
    assert!(row.targeting.is_some());

    cache::init().await.expect("feature flag cache init");
    assert!(is_enabled("billing"), "everyone else gets on");
    assert!(
        !is_enabled_for("billing", &FlagContext::org(held_back)),
        "the held-back org gets off"
    );

    let cleared = store::upsert(&db, "billing", false)
        .await
        .expect("a plain write clears the rules");
    assert!(cleared.targeting.is_none());
}

/// A binary one release back can flip the `enabled` column of a targeted flag, which
/// it believes is the switch. Its last write wins on roll-forward even when it put the
/// column back where it was (on, then off again during an incident), because the
/// document records the `updated_at` it was written with and the old write is later.
#[tokio::test]
async fn a_switch_an_older_binary_wrote_wins_on_roll_forward() {
    use oxy_app::server::feature_flags::registry;
    use oxy_app::server::feature_flags::targeting::{Fallthrough, Targeting};
    use sea_orm::ConnectionTrait;

    let db = common::test_db().await;
    let def = registry::get("billing").expect("billing is registered");
    let targeting = Targeting {
        users: Default::default(),
        orgs: Default::default(),
        fallthrough: Fallthrough::Variant {
            variant: "on".into(),
        },
        salt: None,
    };
    store::write(&db, def, true, Some(&targeting))
        .await
        .expect("write targeted billing");
    assert!(
        billing_switch(&db).await,
        "read back from Postgres, the document's own write is not mistaken for a later one"
    );

    // What the older binary's upsert does: set the column and `updated_at`, nothing else.
    for enabled in [true, false] {
        db.execute_unprepared(&format!(
            "UPDATE feature_flags SET enabled = {enabled}, updated_at = now() WHERE key = 'billing'"
        ))
        .await
        .expect("older binary write");
    }
    assert!(
        !billing_switch(&db).await,
        "the admin's last choice on the older binary was off"
    );
}

/// Billing's kill switch as this binary reads the stored row.
async fn billing_switch(db: &sea_orm::DatabaseConnection) -> bool {
    let row = store::fetch(db, "billing")
        .await
        .unwrap()
        .expect("billing row");
    store::decode(&row).enabled
}
