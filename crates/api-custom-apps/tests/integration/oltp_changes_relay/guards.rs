//! What the relay refuses to trust or to do: a write the host did not sign
//! ships as `unverified`, a store rewound under Airhouse holds its writer,
//! nothing is relayed outside production or with the flag off, and a store
//! where capture was never installed is not woken again.

use oxy_api_custom_apps::oltp_changes::enabled_for;
use oxy_api_custom_apps::oltp_changes::relay::marks::{due, nudge};
use oxy_api_custom_apps::oltp_changes::relay::render::progress_write_sql;
use oxy_api_custom_apps::oltp_changes::relay::{Limits, REDRAIN_EVERY, REDRAIN_MARGIN, RelayStats};
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};

use super::{PRODUCTION, Relay, text, with_relay};
use crate::custom_app_functions_fixture::{Tenant, invocations};
use crate::oltp_changes_fixture::{LEASES_SQL, NOTES_SQL, WRITE, enable_oltp_changes};
use crate::oltp_changes_marks::guard;

/// Remove `t`'s `oltp_changes` override, so the flag is its registry
/// default (off), and load the flag cache from the table.
async fn disable_oltp_changes(t: &Tenant) {
    entity::feature_flag::Entity::delete_by_id(oxy_feature_flags::OLTP_CHANGES.to_string())
        .exec(&t.db)
        .await
        .expect("remove the oltp_changes override");
    oxy_feature_flags::cache::load_once()
        .await
        .expect("load the flag cache");
}

/// The store's owner inserts into a captured table it owns, as someone in
/// `psql` would, beside a host write: the owner's row carries no MAC, so it
/// ships with `_actor = 'unverified'` and no invocation, while the host's
/// keeps the caller and the run.
#[tokio::test]
async fn an_unsigned_write_ships_unverified() {
    with_relay(|r| async move {
        r.app.create_leases_as_store_owner().await;
        let published = r
            .app
            .promote("chg-1", true, &[NOTES_SQL])
            .await
            .expect("capture installs on the table the owner owns");
        let by_hand = format!(
            "insert into {}.leases (id, name) values (1, 'by hand')",
            r.app.schema()
        );
        r.app
            .owner()
            .await
            .batch_execute(&by_hand)
            .await
            .unwrap_or_else(|e| panic!("{by_hand}: {}", oxy_oltp::connect::pg_detail(&e)));
        r.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        assert_eq!(r.waiting(None).await, 2);

        assert_eq!(r.pass().await.shipped, 2);
        let runs = invocations(r.db(), published.app_id, WRITE).await;
        assert_eq!(runs.len(), 1, "{runs:?}");
        let (actor, run) = (r.app.t.guest_id.to_string(), runs[0].id.to_string());
        let rows = r.changes("leases", "id, _actor, CAST(_invocation_id AS VARCHAR)");
        assert_eq!(
            rows,
            [
                [text("1"), text("unverified"), None],
                [text("2"), text(&actor), text(&run)],
            ]
        );
        let unsigned = "count(*) FILTER (WHERE _actor = 'unverified' AND _invocation_id IS NULL)";
        assert_eq!(r.aggregate("leases", unsigned), text("1"));
    })
    .await;
}

/// Airhouse's `_progress` set past the store's intent, as when the store is
/// restored to before a batch Airhouse holds: the next pass ships nothing,
/// declares no batch, and holds the whole writer for a restore.
#[tokio::test]
async fn a_store_rewound_under_airhouse_holds_the_writer() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        assert_eq!(r.pass().await.shipped, 1);
        let (intent, shipped) = r.progress().await;
        assert_eq!(intent, shipped);
        let (generation, _) = r.shipped();
        r.duck
            .exec(&progress_write_sql(&r.writer(), generation, intent + 100));

        r.app
            .write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        let held = RelayStats {
            held: 1,
            ..RelayStats::default()
        };
        assert_eq!(r.pass().await, held, "ships nothing");
        assert_eq!(r.aggregate("leases", "count(*)"), text("1"));
        assert_eq!(r.waiting(None).await, 1, "the row stays in the log");
        assert_eq!(r.progress().await, (intent, shipped), "no batch declared");
        let holds = r.holds().await;
        assert_eq!(holds.len(), 1, "{holds:?}");
        let (writer, table, reason) = &holds[0];
        assert_eq!(
            (writer, table.as_str()),
            (&r.writer(), ""),
            "the whole writer"
        );
        assert!(reason.starts_with("restore detected (Rewound)"), "{reason}");
    })
    .await;
}

/// A change waits in the log. A staging pass, and a production pass with the
/// org's flag off, each return at once: no lease taken, no row shipped,
/// nothing made in Airhouse. With the flag back on, a production pass ships
/// it.
#[tokio::test]
async fn the_relay_is_inert_outside_production_and_without_the_flag() {
    with_relay(|r| async move {
        r.app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote installs capture on leases");
        r.app
            .write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        let limits = Limits::default();

        let staging = r.try_pass("staging", &limits).await;
        assert_eq!(staging, Ok(RelayStats::default()));
        assert_eq!(r.lease_epoch("staging").await, None, "no staging lease");

        disable_oltp_changes(&r.app.t).await;
        assert!(!enabled_for(r.app.org()), "the flag is off");
        let off = r.try_pass(PRODUCTION, &limits).await;
        assert_eq!(off, Ok(RelayStats::default()));
        assert_eq!(r.lease_epoch(PRODUCTION).await, Some(0), "never leased");
        assert_eq!(r.waiting(None).await, 1, "the change still waits");
        assert!(r.tables().is_empty(), "nothing made in Airhouse");

        enable_oltp_changes(&r.app.t).await;
        assert_eq!(r.pass().await.shipped, 1, "the flag back on ships it");
        assert_eq!(r.lease_epoch(PRODUCTION).await, Some(1));
        assert_eq!(r.waiting(None).await, 0);
    })
    .await;
}

/// Whether the sweep would queue a pass for the org's production mark.
async fn is_due(r: &Relay) -> bool {
    due(r.db(), REDRAIN_MARGIN, REDRAIN_EVERY)
        .await
        .expect("due")
        .iter()
        .any(|d| d.org_id == r.app.org() && d.environment == PRODUCTION)
}

/// A flagged org's store where capture was never installed (nothing
/// promoted, so no `stream` row, and so no log row) but whose mark is due,
/// as after a promote that nudged and then failed: the pass ships nothing
/// and drains the mark, so the sweep stops waking the store.
#[tokio::test]
async fn a_store_without_capture_drains_its_mark() {
    with_relay(|r| async move {
        // `mark_all`, in another test, would nudge every mark mid-test.
        let _sweep = guard(r.db(), false).await;
        nudge(r.db(), r.app.org(), PRODUCTION).await.expect("nudge");
        r.db()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE oltp_change_relays SET nudged_at = now() - interval '60 seconds' \
                  WHERE org_id = $1 AND environment = $2",
                [r.app.org().into(), PRODUCTION.into()],
            ))
            .await
            .expect("move the nudge a minute back");
        assert!(is_due(&r).await, "nudged, never drained");

        assert_eq!(r.pass().await, RelayStats::default(), "nothing to relay");
        assert!(
            !is_due(&r).await,
            "drained: the sweep leaves the store asleep"
        );
        assert_eq!(r.backoff().await, (0, false, false));
    })
    .await;
}
