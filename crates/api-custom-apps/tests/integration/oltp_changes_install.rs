//! Change capture installed at a production promote
//! (`oltp_changes::install::install_on_promote`), against an org store
//! provisioned by the real `OltpProvisioner` on the control plane's cluster:
//! the store's owner writes the capture functions, the app's writer attaches
//! the triggers, and every assertion reads the store as its owner.
//!
//! `install_on_promote` is driven directly, as both production promotes call
//! it once the app's OLTP migrations have run; here the writer creates the
//! tables, as those migrations would. The org flag `oltp_changes` is turned on
//! with an org override row, loaded into the flag cache as `serve` loads it at
//! boot, so the control plane is this process's own database
//! (`test_db_with`), not `Fx::create`'s.

use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use futures::FutureExt as _;
use oxy_api_custom_apps::custom_apps_migrations::MigrationError;
use oxy_api_custom_apps::oltp_changes::install::{
    InstallError, InstallSummary, install_on_promote,
};
use oxy_custom_apps::manifest::{OltpChangesConfig, OltpChangesTable};
use oxy_oltp::provider::{LocalProvider, host_from_dsn};
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use oxy_oltp::schema::app_writer_name;
use oxy_oltp::sql::PgSqlExecutor;
use oxy_oltp::{GrantLevel, OltpProvisioner, WriterRef};
use sea_orm::{ActiveModelTrait, ActiveValue::Set};
use serde_json::{Value, json};
use tokio_postgres::Client;
use uuid::Uuid;

use crate::common::{Schema, admin_url, test_db_with};
use crate::oltp_fx::{Fx, seed_org};

/// The table most tests declare, as the app's migrations would create it.
const LEASES: &str = "create table leases (id bigint primary key, name text, contact_email text)";

/// The org's store and the app whose capture is installed in it.
struct Store {
    fx: Fx,
    slug: String,
    writer: WriterRef,
}

/// Run `body` against a fresh org, cleaning its store up even when it panics
/// (`oltp_fx::with_fx`'s reasons). Nothing is provisioned yet.
async fn with_store<F, Fut>(body: F)
where
    F: FnOnce(Arc<Store>) -> Fut,
    Fut: Future<Output = ()>,
{
    // Points `OXY_DATABASE_URL` here, so the flag cache loads this database.
    let db = test_db_with(Schema::CentralOltp).await;
    let admin = admin_url().await;
    let provider = Arc::new(LocalProvider::new(admin.clone(), host_from_dsn(&admin)));
    let org_id = seed_org(&db).await;
    let fx = Fx {
        provisioner: OltpProvisioner::new(
            db.clone(),
            provider.clone(),
            Arc::new(PgSqlExecutor),
            "local",
            oxy_oltp::config::DEFAULT_PG_VERSION,
        ),
        db,
        provider,
        org_id,
        claimant: Uuid::new_v4(),
        minted_roles: std::sync::Mutex::new(Vec::new()),
    };
    // Writer roles are cluster-global: a slug unique to the org keeps two
    // tests from minting the same one.
    let slug = format!("chg-{}", &org_id.simple().to_string()[..12]);
    let writer = WriterRef::app(app_writer_name(&slug).expect("the slug backs a schema"))
        .expect("a valid writer name");
    let store = Arc::new(Store { fx, slug, writer });
    let outcome = std::panic::AssertUnwindSafe(body(store.clone()))
        .catch_unwind()
        .await;
    store.fx.cleanup().await;
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

/// One table under `oltpChanges.tables`.
fn table(name: &str, exclude: &[&str]) -> (String, OltpChangesTable) {
    (
        name.to_string(),
        OltpChangesTable {
            exclude: exclude.iter().map(|c| c.to_string()).collect(),
        },
    )
}

async fn connect(dsn: &str) -> Client {
    oxy_oltp::connect::connect(dsn, "oltp_changes_install test")
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"))
}

impl Store {
    fn org(&self) -> Uuid {
        self.fx.org_id
    }

    /// The org's store and the app's writer, with `ddl` run as that writer.
    async fn provision(&self, ddl: &str) {
        self.fx
            .provisioner
            .provision(self.org())
            .await
            .expect("provision the org's OLTP store");
        self.fx
            .ensure_writer(&self.writer, GrantLevel::ReadWrite)
            .await
            .expect("mint the app's writer");
        self.writer_exec(ddl).await;
    }

    /// Load the flag cache from this test's database, with `oltp_changes` on
    /// for this org alone when `on`, as an admin's org override does.
    async fn load_flags(&self, on: bool) {
        if on {
            let mut orgs = serde_json::Map::new();
            orgs.insert(self.org().to_string(), json!("on"));
            entity::feature_flag::ActiveModel {
                key: Set(oxy_feature_flags::OLTP_CHANGES.to_string()),
                // The registry default; the kill switch is in the document.
                enabled: Set(false),
                updated_at: Set(Utc::now().fixed_offset()),
                targeting: Set(Some(json!({
                    "enabled": true,
                    "orgs": orgs,
                    "fallthrough": { "type": "variant", "variant": "off" },
                }))),
            }
            .insert(&self.fx.db)
            .await
            .expect("write the oltp_changes override");
        }
        oxy_feature_flags::cache::load_once()
            .await
            .expect("load the flag cache");
    }

    /// A promote of a build declaring `tables` (none: no `oltpChanges`).
    async fn install(
        &self,
        tables: Vec<(String, OltpChangesTable)>,
    ) -> Result<InstallSummary, InstallError> {
        let declared = (!tables.is_empty()).then(|| OltpChangesConfig {
            tables: tables.into_iter().collect(),
        });
        install_on_promote(&self.fx.db, self.org(), &self.slug, declared.as_ref()).await
    }

    async fn writer_exec(&self, sql: &str) {
        let conn = resolve_writer_connection_for_org(&self.fx.db, self.org(), &self.writer)
            .await
            .expect("resolve the app's writer");
        connect(&conn.dsn)
            .await
            .batch_execute(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {}", oxy_oltp::connect::pg_detail(&e)));
    }

    async fn owner(&self) -> Client {
        let tenant = self.fx.tenant_row().await.expect("the org's tenant");
        connect(&oxy_oltp::migrator::owner_dsn(&tenant).expect("owner dsn")).await
    }

    /// `table`'s oid.
    async fn relid(&self, table: &str) -> u32 {
        let qualified = format!("{}.{table}", self.writer.schema_name());
        self.owner()
            .await
            .query_one("SELECT to_regclass($1)::oid", &[&qualified])
            .await
            .expect("read the table's oid")
            .get(0)
    }

    /// `(table_name, schema_version, installed)` of each `oxy_changes.tables`
    /// row, by name.
    async fn tables(&self) -> Vec<(String, i32, bool)> {
        self.owner()
            .await
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
        self.owner()
            .await
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

    /// The capture triggers on `table` that are enabled always.
    async fn triggers(&self, table: &str) -> i64 {
        let qualified = format!("{}.{table}", self.writer.schema_name());
        self.owner()
            .await
            .query_one(
                "SELECT count(*) FROM pg_catalog.pg_trigger \
                  WHERE tgrelid = to_regclass($1)::oid \
                    AND left(tgname::text, 12) = 'oxy_capture_' AND tgenabled = 'A'",
                &[&qualified],
            )
            .await
            .expect("count the capture triggers")
            .get(0)
    }

    /// The capture functions generated for `relid`.
    async fn functions(&self, relid: u32) -> i64 {
        let prefix = format!("_oxy_capture_{relid}_");
        self.owner()
            .await
            .query_one(
                "SELECT count(*) FROM pg_catalog.pg_proc \
                  WHERE left(proname::text, length($1)) = $1",
                &[&prefix],
            )
            .await
            .expect("count the capture functions")
            .get(0)
    }

    /// `(op, schema_version, row_image)` of each log row for `table`, in order.
    async fn log(&self, table: &str) -> Vec<(String, i32, Value)> {
        self.owner()
            .await
            .query(
                "SELECT op::text, schema_version, coalesce(row_image::text, 'null') \
                   FROM oxy_changes.log WHERE table_name = $1 ORDER BY seq",
                &[&table],
            )
            .await
            .expect("read oxy_changes.log")
            .iter()
            .map(|r| {
                let image: String = r.get(2);
                (
                    r.get(0),
                    r.get(1),
                    serde_json::from_str(&image).expect("an image is JSON"),
                )
            })
            .collect()
    }

    /// `(table_name, reason, state)` of each `oxy_changes.baselines` row, in
    /// the order they were requested.
    async fn baselines(&self) -> Vec<(String, String, String)> {
        self.owner()
            .await
            .query(
                "SELECT table_name, reason, state FROM oxy_changes.baselines ORDER BY id",
                &[],
            )
            .await
            .expect("read oxy_changes.baselines")
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect()
    }

    async fn stream_rows(&self) -> i64 {
        self.owner()
            .await
            .query_one("SELECT count(*) FROM oxy_changes.stream", &[])
            .await
            .expect("count oxy_changes.stream")
            .get(0)
    }
}

/// With the flag off, a promote declaring tables opens no store connection:
/// for an org with no store at all it answers at once, and for one with a
/// store it writes nothing there and refuses nothing, a missing table
/// included, since it reads no shape.
#[tokio::test]
async fn flag_off_touches_nothing() {
    with_store(|s| async move {
        s.load_flags(false).await;
        // No store: any connection to one would have failed.
        let summary = s
            .install(vec![table("leases", &["contact_email"])])
            .await
            .expect("with the flag off a promote installs nothing");
        assert!(summary.is_empty(), "{summary:?}");

        s.provision(LEASES).await;
        let summary = s
            .install(vec![
                table("leases", &["contact_email"]),
                table("ghosts", &[]),
            ])
            .await
            .expect("with the flag off a promote refuses nothing either");
        assert!(summary.is_empty(), "{summary:?}");
        assert!(s.tables().await.is_empty());
        assert_eq!(s.triggers("leases").await, 0);
        assert_eq!(s.stream_rows().await, 0, "the stream is not seeded");
    })
    .await;
}

/// A declared table gets its functions, its four triggers and a committed
/// `tables` row at version 1, and from then on a write to it is in the log,
/// its excluded column left out.
#[tokio::test]
async fn a_declared_table_is_installed_and_captures() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        let summary = s
            .install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install capture on leases");
        assert_eq!(summary.installed, ["leases"]);
        assert!(
            summary.reshaped.is_empty() && summary.removed.is_empty(),
            "{summary:?}"
        );

        let relid = s.relid("leases").await;
        assert_eq!(s.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(s.shapes(relid).await, [1]);
        assert_eq!(s.triggers("leases").await, 4);
        assert_eq!(s.functions(relid).await, 4);
        assert_eq!(s.stream_rows().await, 1, "the install seeds the stream");

        s.writer_exec(
            "insert into leases (id, name, contact_email) values (1, 'Flat 1', 'a@example.com')",
        )
        .await;
        assert_eq!(
            s.log("leases").await,
            [("i".to_string(), 1, json!({ "id": 1, "name": "Flat 1" }))]
        );
    })
    .await;
}

/// Review focus 3: a declared table that does not exist, or has no primary
/// key, refuses the promote naming every such table, as the author's to fix,
/// before anything changes: an installed table stays installed, whether or
/// not the refused build still declares it.
#[tokio::test]
async fn a_missing_declared_table_refuses_the_publish() {
    with_store(|s| async move {
        s.provision(&format!("{LEASES}; create table notes (body text)"))
            .await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &[])])
            .await
            .expect("install capture on leases");

        let refused = s
            .install(vec![
                table("leases", &[]),
                table("ghosts", &[]),
                table("notes", &[]),
            ])
            .await
            .expect_err("a missing table and a keyless one refuse the promote");
        let message = refused.to_string();
        assert!(matches!(refused, InstallError::Author(_)), "{message}");
        assert!(message.contains("\"ghosts\""), "{message}");
        assert!(message.contains("\"notes\""), "{message}");
        assert!(
            MigrationError::from(refused).is_author_fault(),
            "refused as a migration's author error is: 422"
        );
        assert_eq!(s.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(s.triggers("leases").await, 4);
        assert_eq!(s.triggers("notes").await, 0);

        // A build that drops leases and declares a missing table removes
        // nothing either: the refusal comes first.
        let refused = s
            .install(vec![table("ghosts", &[])])
            .await
            .expect_err("a missing table refuses the promote");
        assert!(matches!(refused, InstallError::Author(_)), "{refused}");
        assert_eq!(s.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(s.triggers("leases").await, 4);
    })
    .await;
}

/// A declared table whose columns use an app type with a cast to `json`
/// installs, and the cast runs inside the capture as the app's image role,
/// not its owner: a cast that reports who it ran as writes that role's name
/// into the image.
#[tokio::test]
async fn an_app_cast_to_json_runs_as_the_image_role() {
    with_store(|s| async move {
        s.provision(
            "create type mood as enum ('ok', 'meh'); \
             create function mood_json(mood) returns json language sql \
               as $f$ select pg_catalog.to_json(current_user::text) $f$; \
             create cast (mood as json) with function mood_json(mood); \
             create table moods (id bigint primary key, mood mood)",
        )
        .await;
        s.load_flags(true).await;
        let summary = s
            .install(vec![table("moods", &[])])
            .await
            .expect("install capture on moods");
        assert_eq!(summary.installed, ["moods"]);
        s.writer_exec("insert into moods values (1, 'ok')").await;

        let tenant = s.fx.tenant_row().await.expect("the org's tenant");
        let writer = app_writer_name(&s.slug).expect("the slug backs a schema");
        let role =
            oxy_oltp::changes::image_role_for(&tenant.provider, &tenant.database_name, &writer);
        assert_eq!(
            s.log("moods").await,
            [("i".to_string(), 1, json!({ "id": 1, "mood": role }))]
        );
    })
    .await;
}

/// A table the next build no longer declares loses its triggers, its
/// functions and its `tables` row; its shapes and the changes already logged
/// stay, and its writes are no longer captured. The table still declared is
/// untouched.
#[tokio::test]
async fn a_removed_table_is_uninstalled_and_its_shapes_stay() {
    with_store(|s| async move {
        s.provision(&format!(
            "{LEASES}; create table landlords (id bigint primary key, name text)"
        ))
        .await;
        s.load_flags(true).await;
        let summary = s
            .install(vec![table("landlords", &[]), table("leases", &[])])
            .await
            .expect("install capture on both");
        assert_eq!(summary.installed, ["landlords", "leases"]);
        let leases = s.relid("leases").await;
        s.writer_exec("insert into leases (id, name) values (1, 'Flat 1')")
            .await;

        let summary = s
            .install(vec![table("landlords", &[])])
            .await
            .expect("remove capture from leases");
        assert_eq!(summary.removed, ["leases"]);
        assert!(
            summary.installed.is_empty() && summary.reshaped.is_empty(),
            "{summary:?}"
        );
        assert_eq!(s.tables().await, [("landlords".to_string(), 1, true)]);
        assert_eq!(s.triggers("leases").await, 0);
        assert_eq!(s.functions(leases).await, 0);
        assert_eq!(
            s.shapes(leases).await,
            [1],
            "its shapes stay, for the history already shipped"
        );
        assert_eq!(s.triggers("landlords").await, 4);

        s.writer_exec("insert into leases (id, name) values (2, 'Flat 2')")
            .await;
        let log = s.log("leases").await;
        assert_eq!(log.len(), 1, "only the write made while captured: {log:?}");
    })
    .await;
}

/// Installing a table again as it stands reports nothing and keeps its
/// version and its one shape; a column a migration adds is a new shape, so the
/// next install records version 2 and captures the column from then on.
#[tokio::test]
async fn reinstalling_an_unchanged_table_keeps_its_schema_version() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install capture on leases");
        let again = s
            .install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install it again");
        assert!(again.is_empty(), "{again:?}");
        let relid = s.relid("leases").await;
        assert_eq!(s.tables().await, [("leases".to_string(), 1, true)]);
        assert_eq!(s.shapes(relid).await, [1]);
        assert_eq!(s.triggers("leases").await, 4);

        s.writer_exec("alter table leases add column floor int")
            .await;
        let reshaped = s
            .install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install the new shape");
        assert_eq!(reshaped.reshaped, ["leases"]);
        assert_eq!(s.tables().await, [("leases".to_string(), 2, true)]);
        assert_eq!(s.shapes(relid).await, [1, 2]);
        s.writer_exec(
            "insert into leases (id, name, contact_email, floor) \
             values (1, 'Flat 1', 'a@example.com', 3)",
        )
        .await;
        assert_eq!(
            s.log("leases").await,
            [(
                "i".to_string(),
                2,
                json!({ "id": 1, "name": "Flat 1", "floor": 3 })
            )]
        );
    })
    .await;
}

fn baseline(table: &str, reason: &str, state: &str) -> (String, String, String) {
    (table.to_string(), reason.to_string(), state.to_string())
}

/// The first install requests the table's opt-in baseline in the transaction
/// that records its shape; the relay (not the install) runs it.
#[tokio::test]
async fn a_first_install_requests_an_opt_in_baseline() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install capture on leases");
        assert_eq!(
            s.baselines().await,
            [baseline("leases", "opt_in", "requested")]
        );
    })
    .await;
}

/// A reshape requests a baseline too. The opt-in request nothing has begun is
/// still `requested`, and covers the new shape, so a second request coalesces
/// into it.
#[tokio::test]
async fn a_reshape_requests_a_baseline() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install capture on leases");
        s.writer_exec("alter table leases add column floor int")
            .await;
        let reshaped = s
            .install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install the new shape");
        assert_eq!(reshaped.reshaped, ["leases"]);
        assert_eq!(
            s.baselines().await,
            [baseline("leases", "opt_in", "requested")]
        );
    })
    .await;
}

/// A baseline already copying when the shape changes copied the old shape: the
/// reshape abandons it and requests another.
#[tokio::test]
async fn a_reshape_after_a_begun_baseline_abandons_it_and_requests_again() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install capture on leases");
        s.owner()
            .await
            .batch_execute(
                "UPDATE oxy_changes.baselines SET state = 'copying', begun_at = now() \
                 WHERE state = 'requested'",
            )
            .await
            .expect("begin the first baseline");
        s.writer_exec("alter table leases add column floor int")
            .await;
        s.install(vec![table("leases", &["contact_email"])])
            .await
            .expect("install the new shape");
        assert_eq!(
            s.baselines().await,
            [
                baseline("leases", "opt_in", "abandoned"),
                baseline("leases", "reshape", "requested"),
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn an_unchanged_reinstall_requests_nothing() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        let declared = || vec![table("leases", &["contact_email"])];
        s.install(declared()).await.expect("install capture");
        s.owner()
            .await
            .batch_execute("UPDATE oxy_changes.baselines SET state = 'complete'")
            .await
            .expect("finish the baseline");
        let again = s.install(declared()).await.expect("install it again");
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(
            s.baselines().await,
            [baseline("leases", "opt_in", "complete")]
        );
    })
    .await;
}

#[tokio::test]
async fn a_removed_table_abandons_its_open_baseline() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &[])])
            .await
            .expect("install capture on leases");
        let removed = s.install(vec![]).await.expect("remove capture");
        assert_eq!(removed.removed, ["leases"]);
        assert_eq!(
            s.baselines().await,
            [baseline("leases", "opt_in", "abandoned")]
        );
    })
    .await;
}

/// Removed, then declared again with its shape unchanged (so no new `shapes`
/// row): it is `New`, so it is baselined again.
#[tokio::test]
async fn a_table_declared_again_is_baselined_again() {
    with_store(|s| async move {
        s.provision(LEASES).await;
        s.load_flags(true).await;
        s.install(vec![table("leases", &[])])
            .await
            .expect("install capture on leases");
        s.install(vec![]).await.expect("remove capture");
        s.install(vec![table("leases", &[])])
            .await
            .expect("declare it again");
        assert_eq!(
            s.baselines().await,
            [
                baseline("leases", "opt_in", "abandoned"),
                baseline("leases", "opt_in", "requested"),
            ]
        );
    })
    .await;
}
