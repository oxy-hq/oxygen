//! The OLTP change stream end to end, through the paths production runs: a
//! promoting `publish` applies the build's OLTP migrations and installs
//! capture on the tables it declares under `oltpChanges`, and a function
//! called through the serve route writes with `ctx.oltp`, the isolate and the
//! host signing its transaction ([`crate::oltp_changes_fixture`]). Every
//! assertion reads the org's store as its owner ([`OwnerReads`]).
//!
//! Who owns a captured table decides who attaches its triggers: a table the
//! app's migrations made is the writer's, one the store owner made in the
//! app's schema (as a workspace's `schemas/*.sql` do) is the owner's, and one
//! any other role owns is refused.

use oxy_api_custom_apps::custom_apps_publish::PublishError;
use oxy_api_custom_apps::oltp_changes::provenance::{self, ChangeSigner};
use oxy_oltp::WriterRef;
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use oxy_oltp::schema::app_writer_name;
use serde_json::{Value, json};
use tokio_postgres::Client;
use uuid::Uuid;

use crate::common::admin_url;
use crate::custom_app_functions_fixture::invocations;
use crate::oltp_changes_fixture::{App, LEASES_SQL, NOTES_SQL, WRITE, connect, with_app};

/// The four capture triggers, by name, each enabled always (`tgenabled`).
fn capture_triggers() -> Vec<(String, String)> {
    ["d", "i", "t", "u"]
        .iter()
        .map(|op| (format!("oxy_capture_{op}"), "A".to_string()))
        .collect()
}

/// One `oxy_changes.log` row, as the relay reads it.
#[derive(Debug, Clone, PartialEq)]
struct Change {
    op: String,
    schema_version: i32,
    key: Value,
    image: Value,
    txid: u64,
    actor: Option<String>,
    invocation: Option<Uuid>,
    provenance: Option<Vec<u8>>,
}

/// What only these tests ask of the app.
impl App {
    /// The store read as its owner, for the assertions.
    async fn as_owner(&self) -> OwnerReads {
        OwnerReads {
            client: self.owner().await,
            schema: self.schema(),
        }
    }

    /// The cluster's admin on the org's database: a superuser, which may give
    /// a table to any role.
    async fn admin(&self) -> Client {
        let database = self.tenant().await.database_name;
        connect(&on_database(&admin_url().await, &database)).await
    }

    /// `leases` created in the app's schema and given to the writer of
    /// another app in the org: a role that is neither the app's writer nor
    /// the store's owner. Returns that role.
    async fn give_leases_to_another_writer(&self) -> String {
        let name = app_writer_name(&format!("{}-x", self.slug)).expect("the slug backs a schema");
        let other = WriterRef::app(name).expect("a valid writer name");
        self.store.ensure_extra_writer(&other, self.workspace).await;
        let other_role = resolve_writer_connection_for_org(&self.t.db, self.org(), &other)
            .await
            .expect("resolve the other writer")
            .role;
        let leases = format!("{}.leases", self.schema());
        self.admin()
            .await
            .batch_execute(&format!(
                "create table {leases} (id bigint primary key, name text); \
                 alter table {leases} owner to \"{other_role}\""
            ))
            .await
            .unwrap_or_else(|e| panic!("give leases away: {}", oxy_oltp::connect::pg_detail(&e)));
        other_role
    }
}

/// The org's store as its owner sees it: the catalog, and `oxy_changes`,
/// which no writer can read.
struct OwnerReads {
    client: Client,
    /// `app_<writer>`.
    schema: String,
}

impl OwnerReads {
    fn qualified(&self, table: &str) -> String {
        format!("{}.{table}", self.schema)
    }

    /// `table`'s owner, as `pg_tables` names it.
    async fn table_owner(&self, table: &str) -> String {
        self.client
            .query_one(
                "SELECT tableowner::text FROM pg_catalog.pg_tables \
                  WHERE schemaname = $1 AND tablename = $2",
                &[&self.schema, &table],
            )
            .await
            .expect("read the table's owner")
            .get(0)
    }

    /// `table`'s oid.
    async fn relid(&self, table: &str) -> u32 {
        self.client
            .query_one("SELECT to_regclass($1)::oid", &[&self.qualified(table)])
            .await
            .expect("read the table's oid")
            .get(0)
    }

    /// `(tgname, tgenabled)` of each trigger on `table` (none when it does
    /// not exist), by name.
    async fn triggers(&self, table: &str) -> Vec<(String, String)> {
        self.client
            .query(
                "SELECT tgname::text, tgenabled::text FROM pg_catalog.pg_trigger \
                  WHERE tgrelid = to_regclass($1)::oid AND NOT tgisinternal ORDER BY tgname",
                &[&self.qualified(table)],
            )
            .await
            .expect("read the table's triggers")
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect()
    }

    /// The capture functions generated for `relid`.
    async fn functions(&self, relid: u32) -> i64 {
        let prefix = format!("_oxy_capture_{relid}_");
        self.client
            .query_one(
                "SELECT count(*) FROM pg_catalog.pg_proc \
                  WHERE left(proname::text, length($1)) = $1",
                &[&prefix],
            )
            .await
            .expect("count the capture functions")
            .get(0)
    }

    /// `(table_name, schema_version, installed)` of each `oxy_changes.tables`
    /// row, by name.
    async fn tables(&self) -> Vec<(String, i32, bool)> {
        self.client
            .query(
                "SELECT table_name, schema_version, installed FROM oxy_changes.tables \
                 ORDER BY table_name",
                &[],
            )
            .await
            .expect("read oxy_changes.tables")
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    }

    /// The versions of `relid`'s recorded shapes, in order.
    async fn shapes(&self, relid: u32) -> Vec<i32> {
        self.client
            .query(
                "SELECT schema_version FROM oxy_changes.shapes WHERE relid = $1 \
                 ORDER BY schema_version",
                &[&relid],
            )
            .await
            .expect("read oxy_changes.shapes")
            .iter()
            .map(|r| r.get(0))
            .collect()
    }

    async fn stream_rows(&self) -> i64 {
        self.client
            .query_one("SELECT count(*) FROM oxy_changes.stream", &[])
            .await
            .expect("count oxy_changes.stream")
            .get(0)
    }

    /// Every log row for `table`, in `seq` order.
    async fn log(&self, table: &str) -> Vec<Change> {
        self.client
            .query(
                "SELECT op::text, schema_version, coalesce(key::text, 'null'), \
                        coalesce(row_image::text, 'null'), txid::text, actor, \
                        invocation::text, provenance \
                   FROM oxy_changes.log WHERE table_name = $1 ORDER BY seq",
                &[&table],
            )
            .await
            .expect("read oxy_changes.log")
            .iter()
            .map(change_of)
            .collect()
    }
}

/// One row of [`OwnerReads::log`]'s query.
fn change_of(r: &tokio_postgres::Row) -> Change {
    let parsed =
        |i: usize| -> Value { serde_json::from_str(&r.get::<_, String>(i)).expect("JSON") };
    let txid: String = r.get(4);
    let invocation: Option<String> = r.get(6);
    Change {
        op: r.get(0),
        schema_version: r.get(1),
        key: parsed(2),
        image: parsed(3),
        txid: provenance::parse_txid(&txid).expect("a transaction id"),
        actor: r.get(5),
        invocation: invocation.map(|id| Uuid::parse_str(&id).expect("a uuid")),
        provenance: r.get(7),
    }
}

/// `url` with its database replaced by `database`.
fn on_database(url: &str, database: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let server = base.rsplit_once('/').map_or(base, |(server, _)| server);
    match query {
        Some(query) => format!("{server}/{database}?{query}"),
        None => format!("{server}/{database}"),
    }
}

/// The table an app's own migration creates is its writer's, so the writer
/// attaches the four capture triggers, each enabled always. A write the host
/// makes (a function's `ctx.oltp.exec` on production) is logged with the
/// caller as actor and the run's invocation, under a MAC that verifies for
/// that row's transaction id and under no other org.
#[tokio::test]
async fn an_app_migration_makes_a_writer_owned_table_and_its_writes_are_signed() {
    with_app(|app| async move {
        let published = app
            .promote("chg-1", true, &[LEASES_SQL])
            .await
            .expect("the promote applies the migration and installs capture");
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, app.writer().await.role);
        assert_eq!(store.triggers("leases").await, capture_triggers());
        assert_eq!(store.tables().await, [("leases".to_string(), 1, true)]);

        app.write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        let log = store.log("leases").await;
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!((log[0].op.as_str(), log[0].schema_version), ("i", 1));
        assert_eq!(log[0].key, json!({ "id": 1 }));
        assert_eq!(log[0].image, json!({ "id": 1, "name": "Flat 1" }));

        let runs = invocations(&app.t.db, published.app_id, WRITE).await;
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_signed_by_the_host(&app, &log[0], runs[0].id);
    })
    .await;
}

/// `change` was signed by the host for the guest's run `invocation` on
/// production: its actor and invocation are the signer's, and its MAC
/// verifies for that row's transaction id under the org's key, and for no
/// other org or transaction.
fn assert_signed_by_the_host(app: &App, change: &Change, invocation: Uuid) {
    let actor = app.t.guest_id.to_string();
    assert_eq!(change.actor.as_deref(), Some(actor.as_str()), "the caller");
    assert_eq!(change.invocation, Some(invocation), "the run");
    let schema = app.schema();
    let signer = ChangeSigner {
        txid: change.txid,
        environment: "production",
        writer: &schema,
        actor: &actor,
        invocation: Some(invocation),
    };
    let mac = change.provenance.as_deref();
    assert_eq!(mac.map(<[u8]>::len), Some(32), "the host's MAC");
    assert!(provenance::verify(app.org(), &signer, mac), "it verifies");
    assert!(
        !provenance::verify(Uuid::new_v4(), &signer, mac),
        "never under another org"
    );
    let elsewhere = ChangeSigner {
        txid: change.txid + 1,
        ..signer
    };
    assert!(
        !provenance::verify(app.org(), &elsewhere, mac),
        "never for another transaction"
    );
}

/// A table the store's owner creates in the app's schema, as a workspace's
/// `schemas/*.sql` do, granted to the writer as the migrator grants it: the
/// writer holds `ALL` on it but does not own it, so the owner attaches its
/// triggers. It is captured like any other, and a build declaring nothing
/// takes the capture down again as the owner, keeping its shapes.
#[tokio::test]
async fn a_workspace_owned_table_in_the_apps_schema_installs_and_captures() {
    with_app(|app| async move {
        let store_owner = app.create_leases_as_store_owner().await;
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, store_owner);

        app.promote("chg-1", true, &[NOTES_SQL])
            .await
            .expect("capture installs on a table the owner owns");
        let relid = store.relid("leases").await;
        assert_eq!(store.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(store.triggers("leases").await, capture_triggers());
        assert_eq!(store.functions(relid).await, 4);

        app.write("insert into leases (id, name) values (1, 'Flat 1')")
            .await;
        let log = store.log("leases").await;
        assert_eq!(log.len(), 1, "{log:?}");
        assert_eq!((log[0].op.as_str(), log[0].schema_version), ("i", 1));
        assert_eq!(log[0].image, json!({ "id": 1, "name": "Flat 1" }));

        app.promote("chg-2", false, &[NOTES_SQL])
            .await
            .expect("a build declaring nothing removes the capture");
        assert!(store.triggers("leases").await.is_empty(), "triggers gone");
        assert!(store.tables().await.is_empty(), "its tables row gone");
        assert_eq!(store.functions(relid).await, 0);
        assert_eq!(store.shapes(relid).await, [1], "its shapes stay");
        app.write("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        assert_eq!(store.log("leases").await, log, "no longer captured");
    })
    .await;
}

/// A declared table some third role owns: neither the writer's session nor
/// the owner's may attach its triggers, so the promote is refused as the
/// author's error, naming the table and its owner, before anything of the
/// capture is installed.
#[tokio::test]
async fn a_table_owned_by_another_role_is_refused() {
    with_app(|app| async move {
        let other_role = app.give_leases_to_another_writer().await;
        let store = app.as_owner().await;
        assert_eq!(store.table_owner("leases").await, other_role);

        let refused = app
            .promote("chg-1", true, &[NOTES_SQL])
            .await
            .expect_err("a table another role owns refuses the promote");
        let PublishError::Migration(error) = &refused else {
            panic!("refused as a migration is: {refused}");
        };
        assert!(error.is_author_fault(), "the author's to fix: {error}");
        let message = error.to_string();
        assert!(message.contains("\"leases\""), "{message}");
        assert!(message.contains(&format!("\"{other_role}\"")), "{message}");

        let relid = store.relid("leases").await;
        assert!(store.tables().await.is_empty(), "nothing registered");
        assert!(store.triggers("leases").await.is_empty(), "no triggers");
        assert_eq!(store.functions(relid).await, 0, "no functions");
        assert_eq!(store.stream_rows().await, 0, "the stream is not seeded");
    })
    .await;
}
