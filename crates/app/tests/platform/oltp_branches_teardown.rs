//! The staging branch's destructive half, on the Neon-shaped fixture from
//! `oltp_branches.rs`: reset, release of one app, deletion with the org, and the
//! guards that keep every one of them off production.

use oxy_oltp::ProvisionerError;
use oxy_oltp::entity::branch_roles::Entity as BranchRoles;
use oxy_oltp::entity::branches::{self as oltp_branches, BranchStatus, Entity as OltpBranches};
use oxy_oltp::entity::tenants::Entity as OltpTenants;
use std::collections::BTreeSet;

use entity::custom_app_migrations;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::oltp_branches::{STAGING, app_with_ledger, ledger_rows, neon, password_of};

/// `(app, filename, checksum)` for every OLTP ledger row under `target`.
async fn ledger_files(db: &DatabaseConnection, target: &str) -> BTreeSet<(Uuid, String, String)> {
    custom_app_migrations::Entity::find()
        .filter(custom_app_migrations::Column::Store.eq("oltp"))
        .filter(custom_app_migrations::Column::Target.eq(target))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.app_id, r.filename, r.checksum))
        .collect()
}

/// A file only staging applied, on top of what `app_with_ledger` seeded.
async fn staging_only_file(db: &DatabaseConnection, app_id: Uuid, target: &str) {
    custom_app_migrations::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        store: ActiveValue::Set("oltp".into()),
        target: ActiveValue::Set(target.to_string()),
        filename: ActiveValue::Set("0002_staging.sql".into()),
        checksum: ActiveValue::Set("def".into()),
        applied_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        applied_by_build: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed a staging-only ledger row");
}

/// A cut copies production's database, so the branch's ledger starts as a
/// copy of production's rows for the org's apps — and no other org's.
#[tokio::test]
async fn a_cut_starts_the_branch_ledger_as_a_copy_of_productions() {
    let n = neon().await;
    let app = app_with_ledger(&n.db, n.org_id, &["production"]).await;
    let elsewhere = crate::oltp_provisioner::seed_org(&n.db).await;
    app_with_ledger(&n.db, elsewhere, &["production"]).await;

    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();

    let target = oxy_oltp::branches::ledger_target(&row.provider_branch_id);
    assert_eq!(
        ledger_files(&n.db, &target).await,
        BTreeSet::from([(app, "0001_init.sql".to_string(), "abc".to_string())]),
    );
}

#[tokio::test]
async fn a_reset_re_cuts_re_mints_and_copies_productions_ledger() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let target = oxy_oltp::branches::ledger_target(&row.provider_branch_id);
    let app = app_with_ledger(&n.db, n.org_id, &["production", &target]).await;
    staging_only_file(&n.db, app, &target).await;
    let elsewhere = crate::oltp_provisioner::seed_org(&n.db).await;
    app_with_ledger(&n.db, elsewhere, &["production"]).await;
    let production_before = ledger_files(&n.db, "production").await;
    let before = n.staging_dsn().await.unwrap().unwrap();

    let reset = n.prov.reset_branch(n.org_id, STAGING).await.expect("reset");

    assert_eq!(n.provider.branch_resets(&row.provider_branch_id), 1);
    assert_eq!(reset.status, BranchStatus::Active);
    assert!(reset.last_reset_at.is_some(), "the age restarts from here");
    assert_eq!(reset.provider_branch_id, row.provider_branch_id);
    let after = n.staging_dsn().await.unwrap().unwrap();
    assert_ne!(
        password_of(&after),
        password_of(&before),
        "the restore brought production's hashes back; the old staging password is void"
    );
    assert_eq!(
        ledger_files(&n.db, &target).await,
        BTreeSet::from([(app, "0001_init.sql".to_string(), "abc".to_string())]),
        "the fresh copy holds what production applied, and not staging's own file"
    );
    assert_eq!(
        ledger_files(&n.db, "production").await,
        production_before,
        "production's ledger is not the branch's to change"
    );
}

#[tokio::test]
async fn a_branch_recorded_as_production_is_never_reset() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let tenant = n.tenant().await;
    let mut poisoned: oltp_branches::ActiveModel = row.clone().into();
    poisoned.provider_branch_id = ActiveValue::Set(tenant.branch_id.clone());
    poisoned.update(&n.db).await.unwrap();

    let err = n.prov.reset_branch(n.org_id, STAGING).await.unwrap_err();

    assert!(
        matches!(err, ProvisionerError::BranchIsProduction { .. }),
        "{err}"
    );
    assert_eq!(n.provider.branch_resets(&tenant.branch_id), 0);
    assert_eq!(
        n.branch().await.unwrap().status,
        BranchStatus::Active,
        "refused before anything changed"
    );
}

#[tokio::test]
async fn releasing_an_app_drops_its_schema_and_role_on_the_branch_too() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();

    n.prov
        .deprovision_writer(n.org_id, &n.writer)
        .await
        .expect("release");

    let on_branch: Vec<String> = n
        .sql
        .on_host(&row.host)
        .into_iter()
        .flat_map(|(_, s)| s)
        .collect();
    assert!(
        on_branch
            .iter()
            .any(|s| s.contains("DROP SCHEMA") && s.contains("app_store_ops")),
        "the app's staging copy must not outlive the app"
    );
    assert!(
        on_branch
            .iter()
            .any(|s| s.contains("DROP ROLE") && s.contains("app_store_ops_rw")),
        "a Neon branch holds its own copy of the role"
    );
    let left: Vec<String> = BranchRoles::find()
        .all(&n.db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.role_name)
        .collect();
    assert_eq!(
        left,
        ["oxy_analyst_ro"],
        "only the writer's branch credential goes"
    );
}

#[tokio::test]
async fn deprovision_deletes_the_branch_with_the_database() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let target = oxy_oltp::branches::ledger_target(&row.provider_branch_id);
    app_with_ledger(&n.db, n.org_id, &[&target]).await;

    n.prov.deprovision(n.org_id).await.expect("deprovision");

    assert_eq!(
        n.provider.branch_count(),
        0,
        "gone provider-side, not just locally"
    );
    assert!(OltpBranches::find().all(&n.db).await.unwrap().is_empty());
    assert!(BranchRoles::find().all(&n.db).await.unwrap().is_empty());
    assert_eq!(ledger_rows(&n.db, &target).await, 0);
}

/// A row that names production must never become a provider delete of it —
/// asserted on what was ASKED of the provider, because the mock (like Neon)
/// refuses to delete a default branch and would otherwise hide a missing guard.
/// On Neon the project delete still goes ahead: it takes the branches anyway.
#[tokio::test]
async fn a_poisoned_row_never_asks_the_provider_to_delete_production() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let tenant = n.tenant().await;
    let mut poisoned: oltp_branches::ActiveModel = row.into();
    poisoned.provider_branch_id = ActiveValue::Set(tenant.branch_id.clone());
    poisoned.update(&n.db).await.unwrap();

    n.prov
        .deprovision(n.org_id)
        .await
        .expect("a poisoned branch row must not keep the project alive");

    assert!(
        !n.provider
            .branch_delete_attempts()
            .contains(&tenant.branch_id),
        "the provider was asked to delete production's branch: {:?}",
        n.provider.branch_delete_attempts()
    );
    assert_eq!(
        n.provider.project_count(),
        0,
        "the project delete still ran"
    );
    assert!(OltpTenants::find().all(&n.db).await.unwrap().is_empty());
}

/// On Neon a branch that will not delete on its own is taken by the project
/// delete, so it must not stop that delete.
#[tokio::test]
async fn a_branch_that_will_not_delete_does_not_keep_the_neon_project_alive() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let target = oxy_oltp::branches::ledger_target(&row.provider_branch_id);
    app_with_ledger(&n.db, n.org_id, &[&target]).await;
    // The next provider call is the branch delete.
    n.provider
        .push_fault(oxy_oltp::ProviderError::Transport("neon blip".into()));

    n.prov.deprovision(n.org_id).await.expect("deprovision");

    assert_eq!(n.provider.project_count(), 0);
    assert_eq!(n.provider.branch_count(), 0, "gone with the project");
    assert!(OltpBranches::find().all(&n.db).await.unwrap().is_empty());
    assert_eq!(
        ledger_rows(&n.db, &target).await,
        0,
        "the branch's ledger goes with it"
    );
}

/// The staff org-delete path: the branch (a copy) goes, production stays.
#[tokio::test]
async fn deleting_only_the_branches_leaves_the_production_database_untouched() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let target = oxy_oltp::branches::ledger_target(&row.provider_branch_id);
    app_with_ledger(&n.db, n.org_id, &["production", &target]).await;
    let production =
        oxy_oltp::resolver::resolve_writer_connection_for_org(&n.db, n.org_id, &n.writer)
            .await
            .unwrap()
            .dsn;

    n.prov
        .delete_branches(n.org_id)
        .await
        .expect("delete branches");

    assert_eq!(n.provider.branch_count(), 0);
    assert!(OltpBranches::find().all(&n.db).await.unwrap().is_empty());
    assert_eq!(ledger_rows(&n.db, &target).await, 0);
    assert_eq!(
        n.provider.project_count(),
        1,
        "the production project stays"
    );
    assert_eq!(n.tenant().await.status.as_str(), "active");
    assert_eq!(ledger_rows(&n.db, "production").await, 1);
    assert_eq!(
        oxy_oltp::resolver::resolve_writer_connection_for_org(&n.db, n.org_id, &n.writer)
            .await
            .unwrap()
            .dsn,
        production,
        "production resolves exactly as before"
    );
}

/// Releasing an app while the branch is mid-reset: on a provider with its own
/// branch roles the branch is about to be re-cut from a production that no
/// longer has the writer, so it is skipped rather than allowed to block.
#[tokio::test]
async fn releasing_an_app_skips_a_branch_that_is_not_active() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let host = row.host.clone();
    let mut resetting: oltp_branches::ActiveModel = row.into();
    resetting.status = ActiveValue::Set(BranchStatus::Resetting);
    resetting.update(&n.db).await.unwrap();
    let before = n.sql.on_host(&host).len();

    n.prov
        .deprovision_writer(n.org_id, &n.writer)
        .await
        .expect("a branch in flux must not block an app's release");

    assert_eq!(
        n.sql.on_host(&host).len(),
        before,
        "nothing ran on the branch"
    );
    let left: Vec<String> = BranchRoles::find()
        .all(&n.db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.role_name)
        .collect();
    assert_eq!(
        left,
        ["oxy_analyst_ro"],
        "its branch credential is forgotten"
    );
}
