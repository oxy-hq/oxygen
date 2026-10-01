//! `airhouse_pool::ExclusiveLease::poison`: a pooled connection left inside a
//! transaction (a preview batch whose `ROLLBACK` failed) is dropped from its
//! slot and never handed out again.
//!
//! The test Postgres stands in for Airhouse's pgwire endpoint — the connector
//! needs only `information_schema.columns` to start — and `SAVEPOINT`, which
//! Postgres refuses outside a transaction, tells a clean connection from one
//! still inside the last holder's transaction.
//!
//! Run with: `cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_pool_poison)'`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use airhouse::AirhouseConnector;
use oxy_app::agentic_wiring::airhouse_pool::{ExclusiveLease, checkout_exclusive};
use oxy_shared::errors::OxyError;
use tokio_postgres::config::Host;

use crate::common::{Schema, fresh_db};

/// Where the stand-in listens, and who to connect as.
struct Target {
    host: String,
    port: u16,
    user: String,
    password: String,
    db: String,
}

fn target(url: &str) -> Target {
    let config: tokio_postgres::Config = url.parse().expect("a postgres url");
    let Some(Host::Tcp(host)) = config.get_hosts().first().cloned() else {
        panic!("{url}: not a TCP host");
    };
    Target {
        host,
        port: config.get_ports().first().copied().unwrap_or(5432),
        user: config.get_user().unwrap_or("postgres").to_string(),
        password: String::from_utf8_lossy(config.get_password().unwrap_or_default()).into_owned(),
        db: config.get_dbname().unwrap_or("postgres").to_string(),
    }
}

/// Check out the one slot of a transaction identity, counting builds.
async fn checkout(target: &Arc<Target>, builds: &Arc<AtomicUsize>) -> ExclusiveLease {
    let (target, builds) = (Arc::clone(target), Arc::clone(builds));
    checkout_exclusive("preview:tx:poison-test".into(), move || async move {
        builds.fetch_add(1, Ordering::SeqCst);
        let t = target.as_ref();
        AirhouseConnector::new(&t.host, t.port, &t.user, &t.password, &t.db)
            .await
            .map(Arc::new)
            .map_err(|e| OxyError::DBError(e.to_string()))
    })
    .await
    .expect("a pooled connection")
}

/// Whether the connection is still inside a transaction.
async fn in_transaction(lease: &ExclusiveLease) -> bool {
    lease
        .connector()
        .execute_statement("SAVEPOINT probe")
        .await
        .is_ok()
}

#[tokio::test]
async fn a_poisoned_slot_is_rebuilt_and_its_transaction_never_handed_on() {
    // One connection per identity, so every checkout lands on the same slot.
    // SAFETY: nextest runs each test in its own process, and this runs before
    // anything else reads the environment.
    unsafe { std::env::set_var("OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY", "1") };
    let (_db, url) = fresh_db(Schema::CentralAirhouse).await;
    let target = Arc::new(target(&url));
    let builds = Arc::new(AtomicUsize::new(0));

    let mut dirty = checkout(&target, &builds).await;
    dirty.connector().execute_statement("BEGIN").await.unwrap();
    assert!(in_transaction(&dirty).await, "BEGIN did not open one");
    dirty.poison();
    drop(dirty);

    let clean = checkout(&target, &builds).await;
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "the poisoned connection was reused"
    );
    assert!(
        !in_transaction(&clean).await,
        "handed a connection inside the last holder's transaction"
    );

    // The control: a slot that is not poisoned hands the same connection on,
    // transaction and all — the hazard poisoning exists for.
    clean.connector().execute_statement("BEGIN").await.unwrap();
    drop(clean);
    let reused = checkout(&target, &builds).await;
    assert_eq!(builds.load(Ordering::SeqCst), 2);
    assert!(in_transaction(&reused).await);
}
