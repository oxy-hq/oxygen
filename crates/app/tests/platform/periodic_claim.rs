//! The periodic driver's workspace claim
//! (`oxy_app::server::router::periodic::claim`), against a real Postgres.
//!
//! This SQL decides which node drives which workspace each interval, so it
//! gates every schedule and monitor scan the Cloud fleet fires. What is worth
//! a database here is what two nodes must never do: drive the same workspace
//! in one interval, or drive one twice. A workspace is made due by moving its
//! `next_tick_at` into the past, which is what the passage of an interval
//! does.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(periodic_claim)'`

use std::collections::BTreeSet;

use chrono::Utc;
use entity::workspaces::WorkspaceStatus;
use entity::{workspace_periodic_ticks, workspaces};
use oxy_app::server::router::periodic::claim::{claim, due_backlog, seed};
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::common::{Schema, test_db_with};

/// The driver interval the claim advances a row by.
const INTERVAL_SECS: i64 = 30;

/// One ready workspace with a path, as the claim's join returns it.
async fn seed_workspace(db: &DatabaseConnection, name: &str) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(Some(format!("/workspace/oxy_data/{name}"))),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(None),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
        default_branch_attempted_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

/// Let an interval pass: every row is due.
async fn an_interval_passes(db: &DatabaseConnection) {
    db.execute_unprepared(
        "UPDATE workspace_periodic_ticks SET next_tick_at = now() - interval '1 second'",
    )
    .await
    .expect("make ticks due");
}

fn ids(claimed: &[oxy_app::server::router::periodic::claim::Claimed]) -> BTreeSet<Uuid> {
    claimed.iter().map(|c| c.workspace_id).collect()
}

#[tokio::test]
async fn concurrent_claims_take_disjoint_workspaces_once_per_interval() {
    let db = test_db_with(Schema::Central).await;
    let a = seed_workspace(&db, "a").await;
    let b = seed_workspace(&db, "b").await;

    // Every workspace gets a row, once: the second seed finds nothing to add.
    assert_eq!(seed(&db, INTERVAL_SECS).await.expect("seed"), 2);
    assert_eq!(seed(&db, INTERVAL_SECS).await.expect("seed again"), 0);

    an_interval_passes(&db).await;
    assert_eq!(due_backlog(&db).await.expect("backlog"), 2);

    // Two nodes claim a batch of one at the same time: each gets a workspace,
    // and never the same one. Whichever statement runs second either skips the
    // row the first still holds locked, or finds it already moved out of the
    // due set — both answers are "the other one".
    let (x, y) = tokio::join!(claim(&db, 1, INTERVAL_SECS), claim(&db, 1, INTERVAL_SECS));
    let (x, y) = (x.expect("claim x"), y.expect("claim y"));
    assert_eq!(x.len(), 1, "a due workspace is claimed");
    assert_eq!(y.len(), 1, "the other due workspace is claimed too");
    assert_ne!(
        x[0].workspace_id, y[0].workspace_id,
        "two concurrent claims must not take the same workspace"
    );
    let mut both = ids(&x);
    both.extend(ids(&y));
    assert_eq!(both, BTreeSet::from([a, b]));

    // The claim carries the workspace columns the tick needs, joined in the
    // same statement, and a fresh row has never been reconciled.
    for c in x.iter().chain(y.iter()) {
        assert!(
            c.path
                .as_deref()
                .is_some_and(|p| p.starts_with("/workspace/oxy_data/"))
        );
        assert_eq!(c.current_revision_id, None);
        assert_eq!(c.last_reconciled_revision_id, None);
        assert_eq!(c.last_reconciled_at, None);
    }

    // Each claimed row moved exactly one interval out from the claim's own
    // clock, so nobody else drives it until the interval has passed.
    let rows = workspace_periodic_ticks::Entity::find()
        .all(&db)
        .await
        .expect("rows");
    assert_eq!(rows.len(), 2);
    for row in &rows {
        let claimed_at = row.claimed_at.expect("the claim stamps claimed_at");
        assert_eq!(
            row.next_tick_at - claimed_at,
            chrono::Duration::seconds(INTERVAL_SECS),
            "next_tick_at advances one interval from the claim"
        );
    }

    // Nothing is due any more, so a third claim — and a bigger batch — gets
    // nothing, and the backlog reads zero.
    assert!(
        claim(&db, 10, INTERVAL_SECS)
            .await
            .expect("claim")
            .is_empty()
    );
    assert_eq!(due_backlog(&db).await.expect("backlog"), 0);

    // Once the interval has passed both are due again, and a batch of ten
    // takes both at once — a single node with no competition drives them all.
    an_interval_passes(&db).await;
    let all = claim(&db, 10, INTERVAL_SECS).await.expect("claim all");
    assert_eq!(ids(&all), BTreeSet::from([a, b]));
}

#[tokio::test]
async fn a_batch_bounds_one_claim_and_leaves_the_rest_due() {
    let db = test_db_with(Schema::Central).await;
    for name in ["a", "b", "c"] {
        seed_workspace(&db, name).await;
    }
    seed(&db, INTERVAL_SECS).await.expect("seed");
    an_interval_passes(&db).await;

    let first = claim(&db, 2, INTERVAL_SECS).await.expect("claim");
    assert_eq!(first.len(), 2, "a claim takes at most its batch");
    assert_eq!(due_backlog(&db).await.expect("backlog"), 1);
    let second = claim(&db, 2, INTERVAL_SECS).await.expect("claim");
    assert_eq!(second.len(), 1, "the next claim takes what is left");
    assert!(ids(&first).is_disjoint(&ids(&second)));
    assert!(
        claim(&db, 2, INTERVAL_SECS)
            .await
            .expect("claim")
            .is_empty()
    );
}
