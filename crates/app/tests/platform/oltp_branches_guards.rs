//! The staging branch's guards, on the Neon-shaped fixture from
//! `oltp_branches.rs`: one operation at a time per `(tenant, branch)` —
//! provision, reset, delete and an app's release — status changes that are
//! compare-and-set, and the resolver's last refusal of a row that names
//! production.

use std::time::Duration;

use oxy_oltp::entity::branch_roles::Entity as BranchRoles;
use oxy_oltp::entity::branches::{self as oltp_branches, BranchStatus, Entity as OltpBranches};
use oxy_oltp::provider::{BranchRequest, OltpProvider};
use oxy_oltp::resolver::ResolveError;
use oxy_oltp::{OltpBranch, ProvisionerError};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, EntityTrait};

use crate::oltp_branches::{Neon, STAGING, neon, neon_on};

/// Two provisions of one org's branch at once — a double-click, two operators —
/// buy one branch and both succeed. Without the `(tenant, kind)` lock both see
/// no row, both create (the provider adopts the second), and the second insert
/// dies on the unique key.
#[tokio::test]
async fn concurrent_provisions_buy_one_branch_and_both_succeed() {
    let n = neon().await;
    n.provider.delay_branch_creates(Duration::from_millis(300));
    two_provisions_buy_one_branch(&n).await;
}

async fn two_provisions_buy_one_branch(n: &Neon) {
    let (a, b) = tokio::join!(
        n.prov.provision_branch(n.org_id, STAGING),
        n.prov.provision_branch(n.org_id, STAGING),
    );

    let (a, b) = (a.expect("first"), b.expect("second"));
    assert_eq!(a.id, b.id, "one row");
    assert_eq!(n.provider.branch_count(), 1, "one branch");
    assert_eq!(OltpBranches::find().all(&n.db).await.unwrap().len(), 1);
}

/// The lock's transaction idles while the provider works. A control plane that
/// kills idle-in-transaction sessions (common on managed Postgres) would end it
/// mid-provision — and with it the lock, silently — so the lock turns that
/// timeout off for its own transaction.
#[tokio::test]
async fn the_lock_outlives_an_idle_in_transaction_timeout() {
    let (db, url) = crate::common::fresh_db(crate::common::Schema::CentralOltp).await;
    let name = url.rsplit('/').next().unwrap().split('?').next().unwrap();
    db.execute_unprepared(&format!(
        "ALTER DATABASE \"{name}\" SET idle_in_transaction_session_timeout = '200ms'"
    ))
    .await
    .unwrap();
    drop(db);
    // A new pool: only sessions opened after the ALTER carry the timeout.
    let db = sea_orm::Database::connect(&url).await.unwrap();
    let n = neon_on(db).await;
    // Each create idles the lock transaction five times past the timeout.
    n.provider.delay_branch_creates(Duration::from_millis(1000));

    two_provisions_buy_one_branch(&n).await;
}

/// Provision reaches the credential step with a row it only read; on a row
/// naming production that step is `ALTER ROLE … PASSWORD` on production's
/// analyst and every writer. Refused before anything runs.
#[tokio::test]
async fn a_provision_over_a_row_naming_production_is_refused() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let tenant = n.tenant().await;
    // Every branch credential gone, so a provision would mint them all again.
    BranchRoles::delete_many().exec(&n.db).await.unwrap();
    let mut poisoned: oltp_branches::ActiveModel = row.into();
    poisoned.provider_branch_id = ActiveValue::Set(tenant.branch_id.clone());
    poisoned.update(&n.db).await.unwrap();
    let before = n.sql.0.lock().unwrap().len();

    let err = n
        .prov
        .provision_branch(n.org_id, STAGING)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ProvisionerError::BranchIsProduction { .. }),
        "{err}"
    );
    assert_eq!(
        n.sql.0.lock().unwrap().len(),
        before,
        "no SQL ran: nothing was re-minted"
    );
    assert!(BranchRoles::find().all(&n.db).await.unwrap().is_empty());
}

/// A write that read the row as `provisioning` must not flip a branch another
/// operation moved to `resetting` — compare-and-set, not last-writer-wins.
#[tokio::test]
async fn a_stale_status_write_cannot_clobber_a_reset() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let mut stale = row.clone();
    stale.status = BranchStatus::Provisioning;
    let mut resetting: oltp_branches::ActiveModel = row.into();
    resetting.status = ActiveValue::Set(BranchStatus::Resetting);
    resetting.update(&n.db).await.unwrap();

    let err = n
        .prov
        .set_branch_status_for_test(stale, BranchStatus::Active)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            ProvisionerError::BranchStateChanged {
                expected: "provisioning",
                found: "resetting",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(n.branch().await.unwrap().status, BranchStatus::Resetting);
}

/// A row that names production is refused at the last door — the resolver —
/// by branch id, and separately by pointing at production's own host and
/// database under an innocent-looking id.
#[tokio::test]
async fn resolution_refuses_a_row_that_names_production() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    let tenant = n.tenant().await;

    let mut by_id: oltp_branches::ActiveModel = row.clone().into();
    by_id.provider_branch_id = ActiveValue::Set(tenant.branch_id.clone());
    by_id.update(&n.db).await.unwrap();
    assert!(matches!(
        n.staging_dsn().await,
        Err(ResolveError::BranchIsProduction(_, OltpBranch::Staging))
    ));

    // The branch's own id again, so only the endpoint can be what refuses.
    let mut by_endpoint: oltp_branches::ActiveModel = row.clone().into();
    by_endpoint.provider_branch_id = ActiveValue::Set(row.provider_branch_id.clone());
    by_endpoint.host = ActiveValue::Set(tenant.host.clone());
    by_endpoint.database_name = ActiveValue::Set(tenant.database_name.clone());
    by_endpoint.update(&n.db).await.unwrap();
    assert_eq!(
        n.branch().await.unwrap().provider_branch_id,
        row.provider_branch_id
    );
    assert!(matches!(
        n.staging_dsn().await,
        Err(ResolveError::BranchIsProduction(..))
    ));

    // Production's endpoint as its pooler, in capitals, with the port spelled
    // out: still production.
    let (label, rest) = tenant.host.split_once('.').expect("a dotted host");
    let spelled = format!("{label}-pooler.{rest}:5432").to_uppercase();
    let mut by_spelling: oltp_branches::ActiveModel = n.branch().await.unwrap().into();
    by_spelling.host = ActiveValue::Set(spelled);
    by_spelling.update(&n.db).await.unwrap();
    assert!(matches!(
        n.staging_dsn().await,
        Err(ResolveError::BranchIsProduction(..))
    ));
}

/// The branch as a reset that has to re-cut it leaves it: gone provider-side,
/// so the reset cuts a new branch under a new id, and slowly. Returns once that
/// reset holds the branch lock — observed as the row turning `resetting`,
/// which the reset writes only after taking it.
async fn a_slow_re_cut_begins(n: &Neon, row: &oltp_branches::Model) {
    let tenant = n.tenant().await;
    let req = BranchRequest {
        project_id: tenant.project_id.clone(),
        parent_branch_id: tenant.branch_id.clone(),
        name: STAGING.provider_name().into(),
        database_name: tenant.database_name.clone(),
        owner_role: tenant.owner_role.clone(),
    };
    n.provider
        .delete_branch(&req, &row.provider_branch_id)
        .await
        .unwrap();
    n.provider.delay_branch_creates(Duration::from_millis(300));
}

async fn until_resetting(n: &Neon) {
    while n.branch().await.map(|b| b.status) != Some(BranchStatus::Resetting) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A delete that waited on a reset deletes the branch the reset left — not
/// the id it read before waiting, which the re-cut replaced.
#[tokio::test]
async fn a_delete_behind_a_reset_deletes_the_re_cut_branch() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    a_slow_re_cut_begins(&n, &row).await;

    let (reset, deleted) = tokio::join!(n.prov.reset_branch(n.org_id, STAGING), async {
        until_resetting(&n).await;
        n.prov.delete_branches(n.org_id).await
    });

    let reset = reset.expect("reset");
    deleted.expect("delete");
    assert_ne!(reset.provider_branch_id, row.provider_branch_id, "re-cut");
    assert_eq!(
        n.provider.branch_count(),
        0,
        "the re-cut branch must not outlive its row"
    );
    assert!(OltpBranches::find().all(&n.db).await.unwrap().is_empty());
}

/// Releasing an app while a reset is in flight waits for it, then drops the
/// app's schema and role on the branch the reset produced.
#[tokio::test]
async fn releasing_an_app_waits_for_a_reset_and_drops_it_on_the_new_copy() {
    let n = neon().await;
    let row = n.prov.provision_branch(n.org_id, STAGING).await.unwrap();
    a_slow_re_cut_begins(&n, &row).await;

    let (reset, released) = tokio::join!(n.prov.reset_branch(n.org_id, STAGING), async {
        until_resetting(&n).await;
        n.prov.deprovision_writer(n.org_id, &n.writer).await
    });

    let reset = reset.expect("reset");
    released.expect("release");
    let on_new_copy: Vec<String> = n
        .sql
        .on_host(&reset.host)
        .into_iter()
        .flat_map(|(_, s)| s)
        .collect();
    assert!(
        on_new_copy
            .iter()
            .any(|s| s.contains("DROP SCHEMA") && s.contains("app_store_ops")),
        "the app's staging copy must go from the branch the reset left"
    );
    assert!(
        BranchRoles::find()
            .all(&n.db)
            .await
            .unwrap()
            .iter()
            .all(|r| !r.role_name.contains("store_ops")),
        "no branch credential outlives the app"
    );
}
