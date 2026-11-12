//! Tests for the boot seam: that a configured row reaches airway's
//! process-wide `GlobalConfig` **without any airway run**, and that a process
//! with no database degrades to airway's own built-ins.
//!
//! DB-backed tests skip (never fail) when `OXY_DATABASE_URL` is unset, per
//! `test_support::test_db` — grep a run for `skipping:` before believing a
//! PASS.
//!
//! # Only one of these installs, and it says so
//!
//! airway's `install` is a process-wide `OnceLock` and
//! `deployment_config::install_once` is `OnceCell`-guarded on top of it, so
//! "has anything installed yet?" is a property of the *process*, not of a
//! test. nextest runs every test in its own process, which is what makes
//! [`the_boot_seam_installs_without_a_run`]'s "nothing installed before me"
//! precondition affordable — it is the only test in this crate that installs.
//!
//! Under a hypothetical shared process (`cargo test`, which this repo does not
//! use) that precondition would fail loudly rather than silently weaken, and
//! [`no_database_degrades_to_airways_own_defaults`] is written to stay
//! meaningful either way: it asserts the installed state is *unchanged* across
//! the no-database call, which is the actual property under test, and only
//! additionally asserts "still nothing installed" when nothing was installed
//! going in.

use agentic_airway::deployment_config::{DeploymentValues, drift};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

use super::{DeploymentTierBoot, install_deployment_tier, install_deployment_tier_from_env};
use crate::test_support::{self, AIRWAY_DEPLOYMENT_LOCK_KEY, AdvisoryLock, SKIP_MSG, test_db};

async fn lock() -> AdvisoryLock {
    let url = test_support::database_url().expect("OXY_DATABASE_URL set (test_db confirmed it)");
    AdvisoryLock::acquire(&url, AIRWAY_DEPLOYMENT_LOCK_KEY).await
}

async fn exec(db: &DatabaseConnection, sql: &str) {
    db.execute_raw(Statement::from_string(
        db.get_database_backend(),
        sql.to_string(),
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Values distinctive enough that finding them installed cannot be a
/// coincidence, and spanning all three transports the gap was about: the
/// deadline, the identity, and the trust store.
fn configured() -> DeploymentValues {
    DeploymentValues {
        timeout_secs: Some(37),
        user_agent: Some("oxy-airway/boot-seam".into()),
        tls_ca_cert: Some("/etc/pki/boot-seam-ca.pem".into()),
        ..Default::default()
    }
}

/// **The gap this seam closes.** A configured row reaches airway's
/// process-wide `GlobalConfig` through the boot call alone — no
/// `run_pipeline`, no airway run, nothing on the queue. That is what makes
/// the settings reach `POST /sources/discover`, which builds a source
/// connector and talks to the vendor outside any load.
///
/// Driven through [`install_deployment_tier_from_env`], the literal call
/// `oxy serve` and `oxy worker` make, so the test exercises the connection
/// resolution too rather than only the half that takes a handle.
///
/// The row is written with a hand-rolled `INSERT` rather than through the
/// admin upsert: the tier's reader is its own hand-written `SELECT` over
/// `COLUMNS` (`agentic-airway` may not depend on `entity`), and going through
/// the writer that shares neither would let a mismatch pass.
#[tokio::test]
async fn the_boot_seam_installs_without_a_run() {
    let Some(db) = test_db().await else {
        println!("{SKIP_MSG}");
        return;
    };
    let _lock = lock().await;
    exec(&db, "DELETE FROM airway_deployment_config").await;

    assert!(
        agentic_airway::installed_values().is_none(),
        "something installed airway's deployment tier before this test — see the \
         module doc; this must be the only installer in the process"
    );

    exec(
        &db,
        "INSERT INTO airway_deployment_config (timeout_secs, user_agent, tls_ca_cert) \
         VALUES (37, 'oxy-airway/boot-seam', '/etc/pki/boot-seam-ca.pem')",
    )
    .await;

    assert_eq!(
        install_deployment_tier_from_env().await,
        DeploymentTierBoot::Installed
    );

    let expected = configured().effective().expect("a well-formed row");
    let installed =
        agentic_airway::installed_values().expect("the boot seam installed nothing at all");
    assert_eq!(
        installed, expected,
        "the configured row did not reach airway's process-wide GlobalConfig"
    );
    assert!(
        drift(&expected, &installed).is_empty(),
        "the admin surface would report drift against the row it just installed"
    );

    // ── One-shot, and quiet about it ───────────────────────────────────────
    //
    // The `install_once` call in `run_pipeline` survives as a fallback for
    // processes with no oxy-app boot seam, so the second call has to be a
    // clean no-op rather than a second installer. Proven by changing the row
    // underneath it: a call that re-read the table would pick the new value
    // up, and a call that re-installed would log the "did NOT take effect"
    // warning. Neither happens — the `OnceCell` short-circuits both.
    exec(
        &db,
        "UPDATE airway_deployment_config SET timeout_secs = 99 WHERE id = 1",
    )
    .await;
    assert_eq!(
        install_deployment_tier(Some(&db)).await,
        DeploymentTierBoot::Installed,
        "a second boot install must report success, not an error"
    );
    assert_eq!(
        agentic_airway::installed_values().as_ref(),
        Some(&expected),
        "the second call re-read the row — the install is supposed to be one-shot \
         per process, which is what makes 'restart to apply' true"
    );

    exec(&db, "DELETE FROM airway_deployment_config").await;
}

/// **No database is a state, not a failure.** `oxy run` works with no database
/// at all, and a transiently unreachable one must not stop a process booting.
/// Either way airway's own compiled-in timeout, retry and TLS settings stay in
/// force — `HttpConfig::default` and `RetryConfig::default` fall back to their
/// built-ins precisely when nothing is installed.
///
/// Needs no database of its own, so it runs (and means something) on a laptop
/// with `OXY_DATABASE_URL` unset — which is the situation it describes.
#[tokio::test]
async fn no_database_degrades_to_airways_own_defaults() {
    let before = agentic_airway::installed_values();

    assert_eq!(
        install_deployment_tier(None).await,
        DeploymentTierBoot::NoDatabase,
        "a process with no database must report that, not fail"
    );

    assert_eq!(
        agentic_airway::installed_values(),
        before,
        "the no-database path installed something — it has no row to install from"
    );
    if before.is_none() {
        assert!(
            agentic_airway::installed_values().is_none(),
            "nothing installed means airway's built-ins are what a connector reads"
        );
    }
}
