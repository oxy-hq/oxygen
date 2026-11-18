//! Invariant 1 — production only ever serves a build staging has served — on
//! every writer of production's pointer, and asked again under the app's row
//! lock.

use std::time::Duration;

use axum::http::StatusCode;
use entity::{app_builds, apps};
use oxy_api_custom_apps::custom_apps_environments::EnvAction;
use oxy_api_custom_apps::custom_apps_promote::{ProductionMove, PromoteError, move_production};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

use super::{fixture_builds, person, pointers, promote, sandbox_build};
use crate::sandbox_agent_token::fixture::{agent, tarball};

/// A person's sandbox build has no staging event. The new route cannot name
/// it; the console's rollback, Make Live and promote latest — which act on
/// whatever build they are given — refuse it `409 staging_never_served`.
#[tokio::test]
async fn every_production_writer_refuses_a_build_staging_never_served() {
    let agent = agent().await;
    let (prod, _) = fixture_builds(&agent).await;
    let slug = agent.app.slug.clone();
    let unstaged = sandbox_build(&agent, "q1", "sbx-q1", tarball(&slug, "unstaged"), true).await;

    let body = json!({ "from": "staging", "build": "sbx-q1" });
    let (status, refused) = promote(&agent, "production", body).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "build_not_served", "{refused}");

    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    let (status, refused) =
        person(&agent, "POST", &uri, Some(json!({ "build_id": unstaged }))).await;
    assert_eq!(status, StatusCode::CONFLICT, "rollback: {refused}");
    assert_eq!(refused["code"], "staging_never_served", "{refused}");

    let ids = Some(json!({ "ids": [agent.app.id] }));
    let (status, batch) = person(&agent, "POST", "/customer-apps/batch/promote-latest", ids).await;
    assert_eq!(status, StatusCode::OK, "{batch}");
    assert_eq!(
        batch["results"][0]["code"], "staging_never_served",
        "{batch}"
    );

    // A draft pointer no publish wrote: Make Live finds no staging event.
    crate::app_environments::set_pointer(
        &agent.t.db,
        agent.app.id,
        &oxy_app_core::custom_app_environment::AppEnvironment::Staging,
        Some(unstaged),
    )
    .await;
    let uri = format!("/customer-apps/{}/publish", agent.app.id);
    let (status, refused) = person(&agent, "POST", &uri, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "make live: {refused}");
    assert_eq!(refused["code"], "staging_never_served", "{refused}");

    assert_eq!(
        pointers(&agent).await.0,
        Some(prod),
        "production never moved"
    );
}

/// Hold the app's row as a concurrent writer would, start a move, change what
/// it decided on, release: the move — which had admitted the build before it
/// waited — asks again under the lock and refuses.
#[tokio::test]
async fn the_move_asks_again_under_the_app_row_lock() {
    let agent = agent().await;
    let db = agent.t.db.clone();
    let (prod, stg) = fixture_builds(&agent).await;
    let mv = ProductionMove {
        app_id: agent.app.id,
        build: stg.into(),
        action: EnvAction::Promote,
        actor: Some(agent.t.guest_id),
        actor_via: None,
        expect_current: Some(Some(prod)),
        refuse_same: true,
        break_listeners: false,
    };

    // Production moves under the lock: the plan's `expect_current` is stale.
    let moved = race(&db, agent.app.id, mv.clone(), |txn| async move {
        crate::app_environments::set_pointer(
            &txn,
            agent.app.id,
            &oxy_app_core::custom_app_environment::AppEnvironment::Production,
            None,
        )
        .await;
        txn
    })
    .await;
    assert!(
        matches!(moved, Err(PromoteError::TargetMoved { .. })),
        "{moved:?}"
    );

    // The build's validation fails under the lock: invariant 1's companion.
    let mv = ProductionMove {
        expect_current: Some(None),
        ..mv
    };
    let moved = race(&db, agent.app.id, mv, |txn| async move {
        app_builds::Entity::update_many()
            .col_expr(app_builds::Column::ValidationStatus, Expr::value("failed"))
            .filter(app_builds::Column::Id.eq(stg))
            .exec(&txn)
            .await
            .expect("fail the build's validation under the lock");
        txn
    })
    .await;
    match moved {
        Err(PromoteError::Blocked(blockers)) => {
            assert_eq!(blockers[0].code, "validation_not_passed", "{blockers:?}");
        }
        other => panic!("expected validation_not_passed, got {other:?}"),
    }
    assert_eq!(pointers(&agent).await.0, None, "production never moved");
}

/// Lock the app's row, start `mv`, check it waits, run `meanwhile` in the
/// locking transaction, commit, and return the move's answer.
pub(super) async fn race<F, Fut>(
    db: &sea_orm::DatabaseConnection,
    app_id: Uuid,
    mv: ProductionMove,
    meanwhile: F,
) -> Result<(), PromoteError>
where
    F: FnOnce(sea_orm::DatabaseTransaction) -> Fut,
    Fut: std::future::Future<Output = sea_orm::DatabaseTransaction>,
{
    let txn = db.begin().await.expect("begin");
    apps::Entity::find_by_id(app_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .expect("lock the app")
        .expect("the app");
    let mover = db.clone();
    let moving = tokio::spawn(async move { move_production(&mover, &mv).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!moving.is_finished(), "the move waits for the app's row");
    meanwhile(txn).await.commit().await.expect("commit");
    moving.await.expect("the move ran")
}
