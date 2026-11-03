//! The environment branch's consistency with the provider across retries:
//! what a resumed cut keeps, what a re-cut forgets, what a reset tells the
//! provider, and what a delete finds that no row recorded. Mock and Local only.

use chrono::{Duration, Utc};
use oxy_oltp::entity::env_branch_roles::{self, Entity as EnvBranchRoles};
use oxy_oltp::entity::env_branches::{self, EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::{EnvBranchKey, provider_name};
use oxy_oltp::provider::{BranchRequest, OltpProvider, ProviderError};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use crate::oltp_branches::{app_with_ledger, neon};
use crate::oltp_env_branches::{branch_ledger, database_exists, key, production_file};
use crate::oltp_provisioner::with_fx;

async fn role_ids(db: &DatabaseConnection, row_id: Uuid) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = EnvBranchRoles::find()
        .filter(env_branch_roles::Column::EnvBranchRowId.eq(row_id))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    ids.sort();
    ids
}

fn named(tenant: &oxy_oltp::entity::tenants::Model, k: &EnvBranchKey) -> BranchRequest {
    BranchRequest::staging(
        tenant.project_id.as_str(),
        tenant.branch_id.as_str(),
        provider_name(k.app_id, &k.env_name),
        tenant.database_name.as_str(),
        tenant.owner_role.as_str(),
    )
}

#[tokio::test]
async fn a_resumed_cut_keeps_the_fork_point_and_copies_the_ledger_as_of_it() {
    let n = neon().await;
    let app = app_with_ledger(&n.db, n.org_id, &[]).await;
    let k = key(n.org_id, app);
    production_file(&n.db, app, "0001_before.sql", Duration::hours(1)).await;
    n.provider.push_fault(ProviderError::Api {
        status: 500,
        message: "neon is having a day".into(),
    });
    n.prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .unwrap_err();
    let failed = oxy_oltp::env_branches::find(&n.db, &k)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(failed.status, EnvBranchStatus::Failed);
    // Applied to production after the failed attempt's fork point: not in a
    // branch whose data is as of that instant.
    custom_file_after(
        &n.db,
        app,
        failed.fork_at.to_utc() + Duration::milliseconds(1),
    )
    .await;

    let row = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("resume");

    assert_eq!(
        row.fork_at, failed.fork_at,
        "the fork point is not re-stamped"
    );
    assert_eq!(row.expires_at, failed.expires_at);
    let id = row.provider_branch_id.expect("cut");
    assert_eq!(branch_ledger(&n.db, &id).await, ["0001_before.sql"]);
    let requests = n.provider.branch_create_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].parent_timestamp,
        Some(failed.fork_at.to_utc()),
        "the provider forks at the recorded instant, not at head"
    );
}

async fn custom_file_after(db: &DatabaseConnection, app: Uuid, at: chrono::DateTime<Utc>) {
    entity::custom_app_migrations::ActiveModel {
        app_id: ActiveValue::Set(app),
        store: ActiveValue::Set("oltp".into()),
        target: ActiveValue::Set("production".into()),
        filename: ActiveValue::Set("0002_after.sql".into()),
        checksum: ActiveValue::Set("abc".into()),
        applied_at: ActiveValue::Set(at.fixed_offset()),
        applied_by_build: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed a production ledger row");
}

#[tokio::test]
async fn an_in_place_reset_moves_the_providers_expiry_with_the_fork() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let cut = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let id = cut.provider_branch_id.clone().expect("cut");
    assert!(n.provider.branch_expiry_updates().is_empty());

    let reset = n.prov.reset_env_branch(&k, None).await.expect("reset");

    assert_eq!(n.provider.branch_resets(&id), 1, "restored in place");
    assert_eq!(
        n.provider.branch_expiry_updates(),
        [(id, reset.expires_at.to_utc())],
        "the provider's expiry follows the new fork, or it reaps the branch early"
    );
    assert_eq!(
        reset.expires_at.to_utc() - reset.fork_at.to_utc(),
        Duration::days(30)
    );

    // A re-cut carries the expiry in its create request instead.
    n.prov
        .reset_env_branch(&k, Some(Utc::now() - Duration::hours(1)))
        .await
        .expect("recut");
    assert_eq!(n.provider.branch_expiry_updates().len(), 1);
    assert!(n.provider.branch_create_requests()[1].expires_at.is_some());
}

#[tokio::test]
async fn a_recut_forgets_the_old_cuts_ledger_and_roles() {
    let n = neon().await;
    let app = app_with_ledger(&n.db, n.org_id, &[]).await;
    production_file(&n.db, app, "0001_init.sql", Duration::hours(2)).await;
    let k = key(n.org_id, app);
    let cut = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let old = cut.provider_branch_id.clone().expect("cut");
    let old_roles = role_ids(&n.db, cut.id).await;
    assert_eq!(branch_ledger(&n.db, &old).await.len(), 1);
    assert!(!old_roles.is_empty());

    let reset = n
        .prov
        .reset_env_branch(&k, Some(Utc::now() - Duration::hours(1)))
        .await
        .expect("recut");

    let new = reset.provider_branch_id.clone().expect("cut");
    assert_ne!(new, old, "a new provider branch");
    assert!(
        branch_ledger(&n.db, &old).await.is_empty(),
        "old ledger gone"
    );
    assert_eq!(branch_ledger(&n.db, &new).await.len(), 1);
    let new_roles = role_ids(&n.db, reset.id).await;
    assert!(!new_roles.is_empty());
    assert!(
        new_roles.iter().all(|id| !old_roles.contains(id)),
        "every role was minted again, none carried over"
    );
}

/// The row of a cut whose answer was never recorded: the provider made the
/// branch, the row has no id.
#[tokio::test]
async fn delete_finds_a_branch_no_row_recorded_by_its_derived_name() {
    for mode in [EnvBranchMode::Empty, EnvBranchMode::Branch] {
        let n = neon().await;
        let k = key(n.org_id, Uuid::new_v4());
        n.provider.push_fault(ProviderError::Api {
            status: 500,
            message: "answer lost".into(),
        });
        n.prov.cut_env_branch(&k, mode, None).await.unwrap_err();
        let failed = oxy_oltp::env_branches::find(&n.db, &k)
            .await
            .unwrap()
            .expect("row");
        assert!(failed.provider_branch_id.is_none());
        let tenant = n.tenant().await;
        let orphan = n
            .provider
            .create_branch(&named(&tenant, &k))
            .await
            .expect("the provider made it anyway");
        assert_eq!(n.provider.branch_count(), 1);

        assert!(n.prov.delete_env_branch(&k).await.expect("delete"));

        assert_eq!(n.provider.branch_count(), 0, "{mode:?}: no orphan left");
        assert_eq!(n.provider.branch_delete_attempts(), [orphan.id]);
        assert!(
            oxy_oltp::env_branches::find(&n.db, &k)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn delete_finds_an_unrecorded_local_branch_database() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let k = key(fx.org_id, Uuid::new_v4());
        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Empty, None)
            .await
            .expect("cut");
        let database = row.database_name.clone().expect("database");
        // The row forgets the id, as after a lost answer.
        let mut forgotten: env_branches::ActiveModel = row.into();
        forgotten.provider_branch_id = ActiveValue::Set(None);
        forgotten.status = ActiveValue::Set(EnvBranchStatus::Failed);
        forgotten.update(&fx.db).await.expect("forget");
        assert!(database_exists(&database).await);

        assert!(fx.provisioner.delete_env_branch(&k).await.expect("delete"));

        assert!(!database_exists(&database).await, "the database is gone");
    })
    .await;
}
