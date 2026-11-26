//! The Lease Desk example (`customer-apps/examples/lease-desk`) end to end.
//! Its own manifest, migration and function are published and promoted as
//! `oxyc publish --promote` publishes them, the function is called through
//! the serve route as the app's frontend calls it, and the relay ships what
//! it wrote. A property manager signs three leases, changes a rent twice,
//! ends one lease and voids another. Airhouse then holds every version of
//! every lease and each lease as it stands, and never the tenant's email,
//! which the manifest excludes.
//!
//! Beside it runs a second app of the same org, a work-order tracker with a
//! writer of its own in the same store:
//! - one pass ships both apps' writes, each to its own schema;
//! - a restore found through the Lease Desk's writer re-baselines both apps;
//! - a resync of the Lease Desk leaves the other app's rows to ship.

use std::sync::Arc;

use oxy_api_custom_apps::oltp_changes::relay::holds::{self, HoldKey, HoldKind};
use oxy_api_custom_apps::oltp_changes::relay::render::{changes_table_name, progress_write_sql};
use oxy_api_custom_apps::oltp_changes::resync::{Discarded, ResyncReport, resync};
use serde_json::{Value, json};

use super::{PRODUCTION, Relay, text, with_relay};
use crate::custom_app_functions_fixture::FunctionSpec;
use crate::oltp_changes_fixture::connect;

/// The example's `oxy-app.json`.
const MANIFEST: &str =
    include_str!("../../../../../customer-apps/examples/lease-desk/oxy-app.json");
/// The example's one function.
const LEASES_JS: &str =
    include_str!("../../../../../customer-apps/examples/lease-desk/functions/leases.js");
/// The example's one migration.
const LEASES_SQL: (&str, &[u8]) = (
    "migrations/0001_leases.sql",
    include_bytes!("../../../../../customer-apps/examples/lease-desk/migrations/0001_leases.sql"),
);

/// The second app's table, as its first migration creates it.
const WORK_ORDERS_SQL: (&str, &[u8]) = (
    "migrations/0001_work_orders.sql",
    b"create table work_orders (id bigint primary key, unit text not null, \
      issue text not null, status text not null);",
);

/// The change table of `leases`: the columns the manifest captures, typed
/// for DuckDB (`tenant_email` is not among them), then the relay's own.
const LEASES_CHANGES: [(&str, &str); 13] = [
    ("lease_ref", "VARCHAR"),
    ("unit", "VARCHAR"),
    ("tenant_name", "VARCHAR"),
    ("rent_cents", "BIGINT"),
    ("starts_on", "DATE"),
    ("ended_on", "DATE"),
    ("_generation", "BIGINT"),
    ("_seq", "BIGINT"),
    ("_txid", "BIGINT"),
    ("_op", "VARCHAR"),
    ("_changed_at", "TIMESTAMP WITH TIME ZONE"),
    ("_actor", "VARCHAR"),
    ("_invocation_id", "UUID"),
];

fn manifest() -> Value {
    serde_json::from_str(MANIFEST).expect("the example's oxy-app.json")
}

/// The example's function, declared as its manifest declares it.
fn leases_fn() -> FunctionSpec {
    FunctionSpec {
        name: "leases",
        manifest: manifest()["functions"]["leases"].clone(),
        js: LEASES_JS,
    }
}

/// What the example's manifest declares beside its functions.
fn declared() -> Value {
    let manifest = manifest();
    json!({
        "migrations": manifest["migrations"],
        "oltpChanges": manifest["oltpChanges"],
    })
}

/// A `sign` request for lease `lease_ref`, from 1 January 2026.
fn sign(lease_ref: &str, unit: &str, tenant: &str, rent_cents: i64) -> Value {
    let email = format!("{}@example.org", tenant.to_lowercase().replace(' ', "."));
    json!({
        "action": "sign", "leaseRef": lease_ref, "unit": unit, "tenantName": tenant,
        "tenantEmail": email, "rentCents": rent_cents, "startsOn": "2026-01-01",
    })
}

fn set_rent(lease_ref: &str, rent_cents: i64) -> Value {
    json!({ "action": "set-rent", "leaseRef": lease_ref, "rentCents": rent_cents })
}

impl Relay {
    /// The example published and promoted, its migration applied, capture
    /// installed on `leases` and the install's baseline shipped.
    async fn lease_desk(&self) {
        self.app
            .promote_bundle("desk-1", &[leases_fn()], declared(), &[LEASES_SQL])
            .await
            .expect("the example publishes, migrates and installs capture");
        self.settle().await;
    }

    /// One request to the example's function: the lease it answered.
    async fn desk(&self, body: Value) -> Value {
        self.app.call("leases", body).await["lease"].clone()
    }

    /// A second app of the same org with `work_orders` captured and its
    /// baseline shipped: a [`Relay`] over it, shipping to the same Airhouse.
    async fn work_orders(&self) -> Relay {
        let app = Arc::new(self.app.sibling("wo").await);
        app.promote_declaring("wo-1", &["work_orders"], &[WORK_ORDERS_SQL])
            .await
            .expect("the second app installs capture on work_orders");
        let other = Relay {
            app,
            duck: self.duck.clone(),
        };
        other.settle().await;
        other
    }

    /// `columns` of `table`'s change table where `filter`, by generation and
    /// `_seq`.
    fn history(&self, table: &str, columns: &str, filter: &str) -> Vec<Vec<Option<String>>> {
        let from = self.qualified(&changes_table_name(table));
        self.duck.read(&format!(
            "SELECT {columns} FROM {from} WHERE {filter} ORDER BY _generation, _seq"
        ))
    }

    /// The Lease Desk's current-state view, every lease by its reference.
    fn leases_now(&self) -> Vec<Vec<Option<String>>> {
        let from = self.qualified("leases");
        self.duck.read(&format!(
            "SELECT lease_ref, unit, tenant_name, rent_cents, CAST(ended_on AS VARCHAR) \
               FROM {from} ORDER BY lease_ref"
        ))
    }

    /// Lease `lease_ref`'s `tenant_email` in the app's store, read as the
    /// app's writer.
    async fn tenant_email(&self, lease_ref: &str) -> Option<String> {
        let writer = connect(&self.app.writer().await.dsn).await;
        let sql = format!(
            "SELECT tenant_email FROM {}.leases WHERE lease_ref = $1",
            self.app.schema()
        );
        writer
            .query_one(sql.as_str(), &[&lease_ref])
            .await
            .expect("read the lease in the app's store")
            .get(0)
    }

    /// Whether the writer has a completed baseline of `table` for `reason`.
    async fn completed(&self, table: &str, reason: &str) -> bool {
        let wanted = (table.to_owned(), reason.to_owned(), "complete".to_owned());
        self.baselines().await.contains(&wanted)
    }
}

/// A row of [`Relay::leases_now`].
fn lease_now(
    lease_ref: &str,
    unit: &str,
    tenant: &str,
    rent: &str,
    ended: Option<&str>,
) -> Vec<Option<String>> {
    vec![
        text(lease_ref),
        text(unit),
        text(tenant),
        text(rent),
        ended.map(str::to_owned),
    ]
}

/// The property manager's year: three leases signed, 4B's rent raised twice,
/// 7C ended at the year's end, and 9A, entered by mistake, voided. Seven
/// changes, each its own call.
async fn a_year_at_the_desk(r: &Relay) {
    let signed = r.desk(sign("L-4B", "4B", "Ada Lovelace", 185_000)).await;
    assert_eq!(signed["lease_ref"], json!("L-4B"), "{signed}");
    assert_eq!(signed["rent_cents"], json!(185_000), "{signed}");
    r.desk(sign("L-7C", "7C", "Grace Hopper", 210_000)).await;
    r.desk(sign("L-9A", "9A", "Alan Turing", 99_000)).await;
    r.desk(set_rent("L-4B", 190_000)).await;
    let raised = r.desk(set_rent("L-4B", 195_000)).await;
    assert_eq!(raised["rent_cents"], json!(195_000), "{raised}");
    r.desk(json!({ "action": "end", "leaseRef": "L-7C", "endedOn": "2026-12-31" }))
        .await;
    r.desk(json!({ "action": "void", "leaseRef": "L-9A" }))
        .await;
}

/// One pass ships the year's seven changes. The change table keeps every
/// version of every lease, in order, each signed with the caller and its run.
/// The view shows the two leases left, 4B at its last rent and 7C ended.
/// The tenant's email stays in the app's store and is in neither Airhouse
/// table.
#[tokio::test]
async fn the_lease_desk_keeps_every_version_of_a_lease_and_each_lease_as_it_stands() {
    with_relay(|r| async move {
        r.lease_desk().await;
        a_year_at_the_desk(&r).await;
        assert_eq!(r.waiting(None).await, 7, "one change a call");

        assert_eq!(r.pass().await.shipped, 7);

        let columns: Vec<(String, String)> = LEASES_CHANGES
            .iter()
            .map(|(name, ty)| (name.to_string(), ty.to_string()))
            .collect();
        assert_eq!(r.columns("leases"), columns, "no tenant_email");
        let rents = r.history(
            "leases",
            "_op, rent_cents",
            "lease_ref = 'L-4B' AND _op IN ('insert', 'update')",
        );
        let rent = |op: &str, cents: &str| vec![text(op), text(cents)];
        assert_eq!(
            rents,
            [
                rent("insert", "185000"),
                rent("update", "190000"),
                rent("update", "195000")
            ],
            "every rent 4B had"
        );
        let voided = r.history("leases", "_op", "lease_ref = 'L-9A'");
        assert_eq!(voided, [[text("insert")], [text("delete")]]);
        let actor = r.app.t.guest_id.to_string();
        let signed = format!(
            "count(*) FILTER (WHERE _actor = '{actor}' AND _invocation_id IS NOT NULL \
               AND _op <> 'snapshot')"
        );
        assert_eq!(
            r.aggregate("leases", &signed),
            text("7"),
            "each by its call"
        );

        assert_eq!(
            r.leases_now(),
            [
                lease_now("L-4B", "4B", "Ada Lovelace", "195000", None),
                lease_now("L-7C", "7C", "Grace Hopper", "210000", Some("2026-12-31")),
            ],
            "the leases as they stand: 9A voided"
        );
        assert_eq!(
            r.tenant_email("L-4B").await.as_deref(),
            Some("ada.lovelace@example.org"),
            "kept in the store"
        );
        assert_eq!(r.waiting(None).await, 0, "the log keeps none of them");
    })
    .await;
}

/// The Lease Desk and a work-order app of the same org write: one pass ships
/// both, each app's tables in its own schema and neither in the other's, and
/// each app's view shows its own rows.
#[tokio::test]
async fn two_apps_of_one_org_ship_in_one_pass_each_to_its_own_schema() {
    with_relay(|r| async move {
        r.lease_desk().await;
        let orders = r.work_orders().await;
        r.desk(sign("L-4B", "4B", "Ada Lovelace", 185_000)).await;
        orders
            .app
            .write("insert into work_orders values (1, '4B', 'Leaking tap', 'open')")
            .await;
        orders
            .app
            .write("update work_orders set status = 'done' where id = 1")
            .await;

        assert_eq!(r.pass().await.shipped, 3, "both apps' writes");

        let desk_tables = r.tables();
        let order_tables = orders.tables();
        assert!(
            desk_tables.contains(&"leases__changes".to_owned()),
            "{desk_tables:?}"
        );
        assert!(
            order_tables.contains(&"work_orders__changes".to_owned()),
            "{order_tables:?}"
        );
        assert!(
            !desk_tables.iter().any(|t| t.starts_with("work_orders")),
            "{desk_tables:?}"
        );
        assert!(
            !order_tables.iter().any(|t| t.starts_with("leases")),
            "{order_tables:?}"
        );
        assert_eq!(
            r.leases_now(),
            [lease_now("L-4B", "4B", "Ada Lovelace", "185000", None)]
        );
        assert_eq!(
            orders.view("work_orders"),
            [[text("1"), text("4B"), text("Leaking tap"), text("done")]]
        );
        for app in [&r, &orders] {
            assert_eq!(app.waiting(None).await, 0, "{} shipped all", app.writer());
            let (intent, shipped) = app.progress().await;
            assert!(
                shipped > 0 && intent == shipped,
                "{}: {intent}, {shipped}",
                app.writer()
            );
        }
    })
    .await;
}

/// Both apps have shipped when the store is restored to before a batch the
/// Lease Desk's Airhouse holds, and each app writes again. The next pass
/// finds the restore through the Lease Desk's writer and bumps the store's
/// generation once. Settled, both apps have a completed `bump` baseline,
/// ship in the new generation, and their views show every row, the other
/// app's included, though its Airhouse was never rewound.
#[tokio::test]
async fn a_restore_found_through_one_app_rebaselines_every_app_of_the_store() {
    with_relay(|r| async move {
        r.lease_desk().await;
        let orders = r.work_orders().await;
        r.desk(sign("L-4B", "4B", "Ada Lovelace", 185_000)).await;
        orders
            .app
            .write("insert into work_orders values (1, '4B', 'Leaking tap', 'open')")
            .await;
        assert_eq!(r.pass().await.shipped, 2);
        let (intent, _) = r.progress().await;
        let old = r.shipped().0;
        r.duck
            .exec(&progress_write_sql(&r.writer(), old, intent + 100));
        r.desk(set_rent("L-4B", 190_000)).await;
        orders
            .app
            .write("update work_orders set status = 'done' where id = 1")
            .await;

        let stats = r.pass().await;
        assert!(stats.bumped, "{stats:?}");
        r.settle().await;
        orders.settle().await;

        assert!(
            r.completed("leases", "bump").await,
            "{:?}",
            r.baselines().await
        );
        assert!(
            orders.completed("work_orders", "bump").await,
            "{:?}",
            orders.baselines().await
        );
        let new = r.shipped().0;
        assert!(new > old, "{old} -> {new}");
        assert_eq!(
            orders.shipped().0,
            new,
            "the other app ships in the new one"
        );
        assert_eq!(
            r.leases_now(),
            [lease_now("L-4B", "4B", "Ada Lovelace", "190000", None)]
        );
        assert_eq!(
            orders.view("work_orders"),
            [[text("1"), text("4B"), text("Leaking tap"), text("done")]]
        );
        assert!(!r.pass().await.bumped, "bumped once");
    })
    .await;
}

/// The Lease Desk's `leases` is held, so its new lease waits while the other
/// app's work order ships. A resync of the Lease Desk discards its held row
/// and releases it; the other app's row written meanwhile is not discarded.
/// Settled, the lease comes back from the resync's baseline, and the other
/// app's view shows both its work orders.
#[tokio::test]
async fn a_resync_of_the_lease_desk_leaves_the_other_apps_rows_to_ship() {
    with_relay(|r| async move {
        r.lease_desk().await;
        let orders = r.work_orders().await;
        let writer = r.writer();
        let key = HoldKey {
            org_id: r.app.org(),
            environment: PRODUCTION,
            writer: &writer,
            table: Some("leases"),
            kind: HoldKind::Table,
        };
        holds::hold(r.db(), key, "held by the test")
            .await
            .expect("hold leases");
        r.desk(sign("L-4B", "4B", "Ada Lovelace", 185_000)).await;
        orders
            .app
            .write("insert into work_orders values (1, '4B', 'Leaking tap', 'open')")
            .await;
        r.pass().await;
        assert_eq!(r.waiting(None).await, 1, "the held lease waits");
        assert_eq!(orders.waiting(None).await, 0, "the other app ships");
        orders
            .app
            .write("insert into work_orders values (2, '7C', 'Broken lock', 'open')")
            .await;

        let report = resync(r.db(), r.app.org(), &r.app.slug, None)
            .await
            .expect("the resync");

        let expected = ResyncReport {
            writer: writer.clone(),
            requested: vec!["leases".to_owned()],
            released: vec!["leases".to_owned()],
            discarded: vec![Discarded {
                table: "leases".to_owned(),
                rows: 1,
            }],
        };
        assert_eq!(report, expected);
        assert_eq!(orders.waiting(None).await, 1, "not the other app's row");
        r.settle().await;
        orders.settle().await;
        assert_eq!(
            r.leases_now(),
            [lease_now("L-4B", "4B", "Ada Lovelace", "185000", None)]
        );
        assert_eq!(
            orders.view("work_orders"),
            [
                [text("1"), text("4B"), text("Leaking tap"), text("open")],
                [text("2"), text("7C"), text("Broken lock"), text("open")],
            ]
        );
    })
    .await;
}
