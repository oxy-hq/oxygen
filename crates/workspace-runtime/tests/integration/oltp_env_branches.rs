//! A custom-app environment's own OLTP branch, cut, reset and deleted by the
//! real `OltpProvisioner`.
//!
//! Two fixtures. `LocalProvider` on the test cluster shows what only a real
//! Postgres can: that the cut carries production's rows, that a reset
//! re-copies, that an empty cut keeps the platform ledger, and that deleting
//! drops the database. `MockProvider` (the Neon-shaped fixture of
//! `oltp_branches.rs`) shows what is asked of the provider and what a provider
//! failure leaves behind. Never a real Neon API.

use chrono::{Duration, Utc};
use entity::custom_app_migrations;
use oxy_oltp::entity::env_branch_roles::Entity as EnvBranchRoles;
use oxy_oltp::entity::env_branches::{self, EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::EnvBranchKey;
use oxy_oltp::provider::{BranchInit, ProviderError};
use oxy_oltp::resolver::{EnvBranchAccept, resolve_env_branch_writer_for_org};
use oxy_oltp::{GrantLevel, ProvisionerError, WriterRef};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter,
};
use uuid::Uuid;

use crate::oltp_branches::{app_with_ledger, neon};
use crate::oltp_provisioner::{Fx, with_fx};

pub(crate) const ENV: &str = "dev-a1";

pub(crate) fn key(org_id: Uuid, app_id: Uuid) -> EnvBranchKey {
    EnvBranchKey {
        org_id,
        app_id,
        env_name: ENV.into(),
    }
}

/// A DSN for the cluster superuser on `database`.
async fn dsn_on(database: &str) -> String {
    let admin = crate::common::admin_url().await;
    let (base, query) = match admin.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (admin.as_str(), String::new()),
    };
    let cut = base.rfind('/').expect("admin DSN has a /dbname path");
    format!("{}/{database}{query}", &base[..cut])
}

async fn exec(dsn: &str, sql: &str) {
    let client = oxy_oltp::connect::connect(dsn, "env branch test")
        .await
        .unwrap_or_else(|e| panic!("connect for {sql}: {e}"));
    client
        .batch_execute(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
}

async fn texts(dsn: &str, sql: &str) -> Vec<String> {
    let client = oxy_oltp::connect::connect(dsn, "env branch test")
        .await
        .expect("connect");
    client
        .query(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|r| r.get(0))
        .collect()
}

pub(crate) async fn notes(dsn: &str) -> Vec<String> {
    texts(dsn, "SELECT note FROM orders ORDER BY id").await
}

pub(crate) async fn database_exists(name: &str) -> bool {
    let admin = crate::common::admin_url().await;
    let client = oxy_oltp::connect::connect(&admin, "env branch test catalog")
        .await
        .expect("connect admin");
    let n: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_database WHERE datname = $1",
            &[&name],
        )
        .await
        .expect("probe")
        .get(0);
    n > 0
}

async fn schema_exists(database: &str, schema: &str) -> bool {
    let dsn = dsn_on(database).await;
    let rows = texts(
        &dsn,
        &format!("SELECT nspname::text FROM pg_namespace WHERE nspname = '{schema}'"),
    )
    .await;
    !rows.is_empty()
}

/// Production writer with an `orders` table holding one row.
pub(crate) async fn production_with_orders(fx: &Fx) -> (WriterRef, String) {
    fx.provisioner
        .provision(fx.org_id)
        .await
        .expect("provision");
    let writer = fx.writer("envbr");
    let production = fx
        .ensure_writer(&writer, GrantLevel::ReadWrite)
        .await
        .expect("writer")
        .dsn;
    exec(
        &production,
        "CREATE TABLE orders (id int PRIMARY KEY, note text); \
         INSERT INTO orders VALUES (1, 'production')",
    )
    .await;
    (writer, production)
}

/// The writer's DSN on the environment's branch, in any served state.
pub(crate) async fn branch_dsn(
    db: &DatabaseConnection,
    k: &EnvBranchKey,
    writer: &WriterRef,
) -> String {
    resolve_env_branch_writer_for_org(
        db,
        k.org_id,
        k.app_id,
        &k.env_name,
        writer,
        EnvBranchAccept::ReadyOrMigrating,
    )
    .await
    .expect("resolve")
    .expect("the environment has a branch")
    .connection
    .dsn
}

pub(crate) async fn branch_ledger(
    db: &DatabaseConnection,
    provider_branch_id: &str,
) -> Vec<String> {
    let mut files: Vec<String> = custom_app_migrations::Entity::find()
        .filter(
            custom_app_migrations::Column::Target
                .eq(oxy_oltp::branches::ledger_target(provider_branch_id)),
        )
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.filename)
        .collect();
    files.sort();
    files
}

pub(crate) async fn production_file(
    db: &DatabaseConnection,
    app_id: Uuid,
    name: &str,
    ago: Duration,
) {
    custom_app_migrations::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        store: ActiveValue::Set("oltp".into()),
        target: ActiveValue::Set("production".into()),
        filename: ActiveValue::Set(name.into()),
        checksum: ActiveValue::Set("abc".into()),
        applied_at: ActiveValue::Set((Utc::now() - ago).fixed_offset()),
        applied_by_build: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed a production ledger row");
}

#[tokio::test]
async fn a_cut_copies_production_and_mints_its_own_row() {
    with_fx(|fx| async move {
        let (writer, production) = production_with_orders(&fx).await;
        let tenant = fx.tenant_row().await.expect("tenant");
        let k = key(fx.org_id, Uuid::new_v4());

        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");

        assert_eq!(row.status, EnvBranchStatus::Migrating);
        assert_ne!(
            row.database_name.as_deref(),
            Some(tenant.database_name.as_str())
        );
        assert_eq!(row.mode, EnvBranchMode::Branch);
        assert_eq!(
            row.expires_at.to_utc() - row.fork_at.to_utc(),
            Duration::days(30)
        );
        let dsn = branch_dsn(&fx.db, &k, &writer).await;
        assert_eq!(notes(&dsn).await, ["production"], "a copy of production");
        exec(&dsn, "INSERT INTO orders VALUES (2, 'branch')").await;
        assert_eq!(notes(&production).await, ["production"], "writes stay put");

        let again = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Empty, None)
            .await
            .expect("a second cut");
        assert_eq!(again, row, "a migrating branch is returned untouched");
    })
    .await;
}

#[tokio::test]
async fn ledger_copy_stops_at_the_fork_point() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let app = app_with_ledger(&fx.db, fx.org_id, &[]).await;
        production_file(&fx.db, app, "0001_before.sql", Duration::hours(2)).await;
        production_file(&fx.db, app, "0002_after.sql", Duration::minutes(10)).await;

        let row = fx
            .provisioner
            .cut_env_branch(
                &key(fx.org_id, app),
                EnvBranchMode::Branch,
                Some(Utc::now() - Duration::hours(1)),
            )
            .await
            .expect("cut");

        let id = row.provider_branch_id.expect("cut");
        assert_eq!(branch_ledger(&fx.db, &id).await, ["0001_before.sql"]);
    })
    .await;
}

/// Production's two `oxy_meta` ledgers, as `(table, rows)` sorted.
async fn meta_rows(database: &str) -> Vec<String> {
    let dsn = dsn_on(database).await;
    let mut rows = texts(
        &dsn,
        "SELECT 's:' || file_path || ':' || checksum FROM oxy_meta.schema_migrations \
         UNION ALL SELECT 'a:' || app_id || ':' || version || ':' || name \
         FROM oxy_meta.app_migrations",
    )
    .await;
    rows.sort();
    rows
}

#[tokio::test]
async fn an_empty_cut_has_tables_and_no_rows_and_production_s_platform_ledger() {
    with_fx(|fx| async move {
        let (writer, _production) = production_with_orders(&fx).await;
        let tenant = fx.tenant_row().await.expect("tenant");
        let admin_prod = dsn_on(&tenant.database_name).await;
        exec(
            &admin_prod,
            "INSERT INTO oxy_meta.schema_migrations (file_path, checksum) VALUES ('m1.sql', 'abc'); \
             INSERT INTO oxy_meta.app_migrations (app_id, version, name, checksum) \
             VALUES ('00000000-0000-0000-0000-0000000000a1', 1, 'init', 'def')",
        )
        .await;
        let k = key(fx.org_id, Uuid::new_v4());

        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Empty, None)
            .await
            .expect("cut");

        assert_eq!(row.status, EnvBranchStatus::Migrating);
        let dsn = branch_dsn(&fx.db, &k, &writer).await;
        assert!(notes(&dsn).await.is_empty(), "the table is there, empty");
        let branch_db = row.database_name.clone().expect("database");
        let production_meta = meta_rows(&tenant.database_name).await;
        assert!(production_meta.len() >= 2, "{production_meta:?}");
        assert_eq!(meta_rows(&branch_db).await, production_meta);

        // What a Neon schema-only branch looks like: structure, no ledger rows.
        let admin_branch = dsn_on(&branch_db).await;
        exec(
            &admin_branch,
            "DELETE FROM oxy_meta.schema_migrations; DELETE FROM oxy_meta.app_migrations",
        )
        .await;
        assert!(meta_rows(&branch_db).await.is_empty());
        fx.provisioner
            .copy_env_meta_ledger_for_test(&k)
            .await
            .expect("copy");
        assert_eq!(meta_rows(&branch_db).await, production_meta, "copied back");
        fx.provisioner
            .copy_env_meta_ledger_for_test(&k)
            .await
            .expect("copy again");
        assert_eq!(meta_rows(&branch_db).await, production_meta, "idempotent");
    })
    .await;
}

#[tokio::test]
async fn reset_without_at_recopies_production_and_moves_the_fork() {
    with_fx(|fx| async move {
        let (writer, production) = production_with_orders(&fx).await;
        let k = key(fx.org_id, Uuid::new_v4());
        let cut = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        let dsn = branch_dsn(&fx.db, &k, &writer).await;
        exec(&dsn, "INSERT INTO orders VALUES (2, 'branch')").await;
        exec(&production, "INSERT INTO orders VALUES (3, 'later')").await;

        let reset = fx
            .provisioner
            .reset_env_branch(&k, None)
            .await
            .expect("reset");

        assert_eq!(reset.status, EnvBranchStatus::Migrating);
        assert!(reset.fork_at > cut.fork_at, "the fork moved");
        assert!(reset.last_reset_at.is_some());
        let dsn = branch_dsn(&fx.db, &k, &writer).await;
        assert_eq!(notes(&dsn).await, ["production", "later"]);
    })
    .await;
}

#[tokio::test]
async fn reset_at_a_time_recuts() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let cut = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let first = cut.provider_branch_id.clone().expect("cut");

    let reset = n
        .prov
        .reset_env_branch(&k, Some(Utc::now() - Duration::hours(1)))
        .await
        .expect("reset");

    let requests = n.provider.branch_create_requests();
    assert_eq!(requests.len(), 2, "a second create, not a restore");
    assert!(requests[0].parent_timestamp.is_none());
    assert!(requests[1].parent_timestamp.is_some());
    assert_eq!(n.provider.branch_resets(&first), 0);
    assert_eq!(n.provider.branch_delete_attempts(), [first]);
    assert_eq!(reset.status, EnvBranchStatus::Migrating);
    assert!(
        reset.fork_at < cut.fork_at,
        "the fork is the time asked for"
    );

    // And without `at` the branch is restored in place instead.
    let id = reset.provider_branch_id.clone().expect("cut");
    n.prov.reset_env_branch(&k, None).await.expect("restore");
    assert_eq!(n.provider.branch_resets(&id), 1);
    assert_eq!(n.provider.branch_create_requests().len(), 2);
}

#[tokio::test]
async fn an_empty_cut_asks_the_provider_for_no_expiry_and_no_rows() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let empty = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Empty, None)
        .await
        .expect("cut");
    let full = n
        .prov
        .cut_env_branch(&key(n.org_id, Uuid::new_v4()), EnvBranchMode::Branch, None)
        .await
        .expect("cut");

    let requests = n.provider.branch_create_requests();
    assert_eq!(requests[0].expires_at, None, "an empty branch never lapses");
    assert_eq!(requests[0].init, BranchInit::SchemaOnly);
    assert_eq!(requests[1].expires_at, Some(full.expires_at.to_utc()));
    assert_eq!(requests[1].init, BranchInit::ParentData);
    // The row still records the 30 days (`expires_at` is NOT NULL).
    assert_eq!(
        empty.expires_at.to_utc() - empty.fork_at.to_utc(),
        Duration::days(30)
    );

    // A re-cut of an empty branch is a delete and a create, still unexpiring.
    n.prov.reset_env_branch(&k, None).await.expect("reset");
    let requests = n.provider.branch_create_requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].expires_at, None);
    assert_eq!(requests[2].init, BranchInit::SchemaOnly);
}

#[tokio::test]
async fn delete_removes_the_database_and_the_ledger() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let app = app_with_ledger(&fx.db, fx.org_id, &["production"]).await;
        let k = key(fx.org_id, app);
        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        let id = row.provider_branch_id.clone().expect("cut");
        let database = row.database_name.clone().expect("database");
        assert!(database_exists(&database).await);
        assert_eq!(branch_ledger(&fx.db, &id).await.len(), 1);

        assert!(fx.provisioner.delete_env_branch(&k).await.expect("delete"));

        assert!(!database_exists(&database).await, "the database is gone");
        assert!(branch_ledger(&fx.db, &id).await.is_empty());
        assert!(
            oxy_oltp::env_branches::find(&fx.db, &k)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(EnvBranchRoles::find().count(&fx.db).await.unwrap(), 0);
        assert!(
            !fx.provisioner.delete_env_branch(&k).await.expect("again"),
            "a missing row is Ok(false)"
        );
    })
    .await;
}

#[tokio::test]
async fn deprovision_deletes_env_branches_too() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let k = key(fx.org_id, Uuid::new_v4());
        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        let database = row.database_name.clone().expect("database");
        assert!(database_exists(&database).await);

        fx.provisioner
            .deprovision(fx.org_id)
            .await
            .expect("deprovision");

        assert!(!database_exists(&database).await, "no stranded database");
        assert_eq!(env_branches::Entity::find().count(&fx.db).await.unwrap(), 0);
    })
    .await;
}

#[tokio::test]
async fn a_writer_released_after_the_cut_is_dropped_on_the_env_branch() {
    with_fx(|fx| async move {
        let (writer, _production) = production_with_orders(&fx).await;
        let k = key(fx.org_id, Uuid::new_v4());
        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        let database = row.database_name.clone().expect("database");
        assert!(schema_exists(&database, &writer.schema_name()).await);

        fx.provisioner
            .deprovision_writer(fx.org_id, &writer)
            .await
            .expect("an app's release must work with an environment branch in place");

        assert!(
            !schema_exists(&database, &writer.schema_name()).await,
            "the app's schema goes from the environment's branch too"
        );
    })
    .await;
}

#[tokio::test]
async fn a_failed_provider_call_marks_the_row_failed() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    n.provider.push_fault(ProviderError::Api {
        status: 500,
        message: "neon is having a day".into(),
    });

    let err = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("neon is having a day"), "{err}");
    let row = oxy_oltp::env_branches::find(&n.db, &k)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.status, EnvBranchStatus::Failed);
    assert!(
        row.error
            .as_deref()
            .is_some_and(|e| e.contains("neon is having a day"))
    );
    assert!(row.provider_branch_id.is_none());

    // The next attempt resumes, and clears the error.
    let row = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("retry");
    assert_eq!(row.status, EnvBranchStatus::Migrating);
    assert!(row.error.is_none());
}

#[tokio::test]
async fn a_fork_outside_the_history_is_refused_before_any_row_exists() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    for at in [
        Utc::now() + Duration::hours(1),
        Utc::now() - Duration::days(3),
    ] {
        let err = n
            .prov
            .cut_env_branch(&k, EnvBranchMode::Branch, Some(at))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProvisionerError::ForkOutsideHistory(_)),
            "{err}"
        );
    }
    assert!(
        oxy_oltp::env_branches::find(&n.db, &k)
            .await
            .unwrap()
            .is_none()
    );
    assert!(n.provider.branch_create_requests().is_empty());
}

#[tokio::test]
async fn a_row_naming_production_is_never_reset_or_deleted() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let row = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let tenant = n.tenant().await;
    let mut poisoned: env_branches::ActiveModel = row.into();
    poisoned.provider_branch_id = ActiveValue::Set(Some(tenant.branch_id.clone()));
    poisoned.update(&n.db).await.expect("poison");

    let reset = n.prov.reset_env_branch(&k, None).await.unwrap_err();
    let delete = n.prov.delete_env_branch(&k).await.unwrap_err();

    for err in [reset, delete] {
        assert!(
            matches!(err, ProvisionerError::EnvBranchIsProduction { .. }),
            "{err}"
        );
    }
    assert!(
        n.provider.branch_delete_attempts().is_empty(),
        "the provider was never asked to touch production"
    );
    assert_eq!(n.provider.branch_resets(&tenant.branch_id), 0);
}
