//! A branch an earlier attempt left under an environment's derived name is
//! never adopted where it would serve the wrong data: a cut that changes mode
//! deletes it first, and so does a reset that falls back to a fresh create.
//! Mock and Local only.

use oxy_oltp::entity::env_branches::{self, EnvBranchMode, EnvBranchStatus};
use oxy_oltp::env_branches::{EnvBranchKey, provider_name};
use oxy_oltp::provider::{BranchInit, BranchRequest, OltpProvider, ProviderError};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use crate::oltp_branches::neon;
use crate::oltp_env_branches::{branch_dsn, key, notes, production_with_orders};
use crate::oltp_provisioner::with_fx;

/// The row as a cut that failed after the provider answered leaves it —
/// `forget_id`: as one whose answer was never recorded.
async fn fail(db: &DatabaseConnection, row: env_branches::Model, forget_id: bool) {
    let mut failed: env_branches::ActiveModel = row.into();
    failed.status = ActiveValue::Set(EnvBranchStatus::Failed);
    if forget_id {
        failed.provider_branch_id = ActiveValue::Set(None);
    }
    failed.update(db).await.expect("fail the row");
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

/// A `branch` cut that failed, picked up again as `empty`: the data branch
/// under the name is deleted — found by the recorded id, or by name when the
/// answer was never recorded — and a schema-only one is cut in its place.
#[tokio::test]
async fn a_cut_in_another_mode_deletes_the_earlier_attempts_branch() {
    for forget_id in [false, true] {
        let n = neon().await;
        let k = key(n.org_id, Uuid::new_v4());
        let first = n
            .prov
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        let old = first.provider_branch_id.clone().expect("cut");
        fail(&n.db, first, forget_id).await;

        let row = n
            .prov
            .cut_env_branch(&k, EnvBranchMode::Empty, None)
            .await
            .expect("cut in the other mode");

        let new = row.provider_branch_id.clone().expect("cut");
        assert_ne!(new, old, "forget_id={forget_id}: never the data branch");
        assert_eq!(row.mode, EnvBranchMode::Empty);
        assert_eq!(row.status, EnvBranchStatus::Migrating);
        assert_eq!(
            n.provider.branch_init(&new),
            Some(BranchInit::SchemaOnly),
            "holds none of production's rows"
        );
        assert_eq!(n.provider.branch_delete_attempts(), [old.clone()]);
        assert_eq!(
            n.provider.branch_init(&old),
            None,
            "the data branch is gone"
        );
        assert_eq!(n.provider.branch_count(), 1);
    }
}

/// The same on a real Postgres: the sandbox that asked for `empty` sees no
/// production row, though a `branch` cut copied them under the same name.
#[tokio::test]
async fn an_empty_cut_after_a_failed_branch_cut_holds_no_production_rows() {
    with_fx(|fx| async move {
        let (writer, _production) = production_with_orders(&fx).await;
        let k = key(fx.org_id, Uuid::new_v4());
        let first = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Branch, None)
            .await
            .expect("cut");
        assert_eq!(
            notes(&branch_dsn(&fx.db, &k, &writer).await).await,
            ["production"]
        );
        fail(&fx.db, first, false).await;

        let row = fx
            .provisioner
            .cut_env_branch(&k, EnvBranchMode::Empty, None)
            .await
            .expect("cut in the other mode");

        assert_eq!(row.mode, EnvBranchMode::Empty);
        let dsn = branch_dsn(&fx.db, &k, &writer).await;
        assert!(notes(&dsn).await.is_empty(), "no production rows");
    })
    .await;
}

/// A reset of a row that recorded no branch creates one — after deleting the
/// stale branch an unrecorded answer left under the name, never adopting it.
#[tokio::test]
async fn a_reset_with_no_recorded_branch_deletes_a_stale_one_first() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    n.provider.push_fault(ProviderError::Api {
        status: 500,
        message: "answer lost".into(),
    });
    n.prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .unwrap_err();
    let stale = n
        .provider
        .create_branch(&named(&n.tenant().await, &k))
        .await
        .expect("the provider made it anyway");

    let row = n.prov.reset_env_branch(&k, None).await.expect("reset");

    let new = row.provider_branch_id.expect("cut");
    assert_ne!(new, stale.id, "a fresh branch, not the stale one");
    assert_eq!(n.provider.branch_delete_attempts(), [stale.id]);
    assert_eq!(n.provider.branch_count(), 1);
}

/// A restore whose branch the provider lost falls back to a create — after
/// deleting whatever else is under the name.
#[tokio::test]
async fn a_reset_whose_branch_vanished_deletes_a_stale_one_first() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let cut = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let recorded = cut.provider_branch_id.expect("cut");
    let req = named(&n.tenant().await, &k);
    n.provider
        .delete_branch(&req, &recorded)
        .await
        .expect("gone provider-side");
    let stale = n.provider.create_branch(&req).await.expect("stale");

    let row = n.prov.reset_env_branch(&k, None).await.expect("reset");

    let new = row.provider_branch_id.expect("cut");
    assert_ne!(new, stale.id, "a fresh branch, not the stale one");
    assert_eq!(n.provider.branch_resets(&recorded), 0, "nothing to restore");
    assert_eq!(n.provider.branch_delete_attempts(), [recorded, stale.id]);
    assert_eq!(n.provider.branch_count(), 1);
}

/// A re-cut (a reset with `at`) deletes the recorded branch and then whatever
/// an earlier re-cut left under the name before it failed to record it — that
/// branch holds data from its own instant, so adopting it would seed a ledger
/// newer than its data.
#[tokio::test]
async fn a_recut_deletes_an_earlier_recuts_unrecorded_branch() {
    let n = neon().await;
    let k = key(n.org_id, Uuid::new_v4());
    let cut = n
        .prov
        .cut_env_branch(&k, EnvBranchMode::Branch, None)
        .await
        .expect("cut");
    let recorded = cut.provider_branch_id.expect("cut");
    // The earlier re-cut: deleted the recorded branch, created its own, and
    // failed before the row learned of it.
    let req = named(&n.tenant().await, &k);
    n.provider
        .delete_branch(&req, &recorded)
        .await
        .expect("the earlier re-cut's delete");
    let stale = n.provider.create_branch(&req).await.expect("stale");

    let at = chrono::Utc::now() - chrono::Duration::minutes(5);
    let row = n.prov.reset_env_branch(&k, Some(at)).await.expect("reset");

    let new = row.provider_branch_id.expect("cut");
    assert_ne!(new, stale.id, "a fresh branch, not the stale one");
    assert_eq!(
        n.provider.branch_delete_attempts(),
        [recorded.clone(), recorded, stale.id]
    );
    assert_eq!(n.provider.branch_count(), 1);
}
