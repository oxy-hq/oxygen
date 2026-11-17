//! A move aimed at the draft or the newest build names it **under the app's
//! row lock**: a draft publish that lands after the caller read the app (and
//! before the lock) is the build made live, not the one it replaced.

use chrono::{Duration, Utc};
use entity::app_builds;
use oxy_api_custom_apps::custom_apps_environments::EnvAction;
use oxy_api_custom_apps::custom_apps_promote::{BuildRef, ProductionMove, move_production};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::invariant::race;
use super::{draft, events, fixture_builds, pointers};
use crate::sandbox_agent_token::fixture::{Agent, agent, tarball};

/// `agent-newer`, a draft a person publishes — staging serves it, so it is
/// admissible — with staging's pointer put back on `stg`: as if the publish
/// had not landed yet. Returns the newer build.
async fn newer_draft_not_yet_landed(agent: &Agent, stg: Uuid) -> Uuid {
    let slug = agent.app.slug.clone();
    let newer = draft(agent, "agent-newer", tarball(&slug, "newer")).await;
    set_draft(&agent.t.db, agent.app.id, stg).await;
    newer
}

async fn set_draft<C: sea_orm::ConnectionTrait>(db: &C, app_id: Uuid, build: Uuid) {
    crate::app_environments::set_pointer(
        db,
        app_id,
        &oxy_app_core::custom_app_environment::AppEnvironment::Staging,
        Some(build),
    )
    .await;
}

fn move_to(agent: &Agent, build: BuildRef) -> ProductionMove {
    ProductionMove {
        app_id: agent.app.id,
        build,
        action: EnvAction::Promote,
        actor: Some(agent.t.guest_id),
        actor_via: None,
        expect_current: None,
        refuse_same: false,
    }
}

/// Without a race, `Draft` is the build staging serves.
#[tokio::test]
async fn the_draft_is_the_build_staging_serves_when_nothing_races() {
    let agent = agent().await;
    let (_, stg) = fixture_builds(&agent).await;
    move_production(&agent.t.db, &move_to(&agent, BuildRef::Draft))
        .await
        .expect("make the draft live");
    assert_eq!(pointers(&agent).await.0, Some(stg));
}

/// The caller read the draft (`agent-stg`); a draft publish lands before the
/// move gets the row lock. Production moves to the newer draft, and its
/// event names it.
#[tokio::test]
async fn a_draft_published_between_the_read_and_the_lock_wins() {
    let agent = agent().await;
    let (_, stg) = fixture_builds(&agent).await;
    let newer = newer_draft_not_yet_landed(&agent, stg).await;
    let app_id = agent.app.id;

    let mv = move_to(&agent, BuildRef::Draft);
    race(&agent.t.db, app_id, mv, |txn| async move {
        set_draft(&txn, app_id, newer).await;
        txn
    })
    .await
    .expect("the move follows the draft");

    assert_eq!(pointers(&agent).await.0, Some(newer), "the newer draft");
    let last = events(&agent).await.pop().expect("an event");
    assert_eq!(
        last,
        ("production".to_string(), Some(newer), "promote".to_string())
    );
}

/// The same for *promote latest*: a build that becomes the newest after the
/// read is the one promoted.
#[tokio::test]
async fn a_build_that_becomes_the_newest_before_the_lock_wins() {
    let agent = agent().await;
    let (_, stg) = fixture_builds(&agent).await;
    let newer = newer_draft_not_yet_landed(&agent, stg).await;
    let app_id = agent.app.id;
    let db = &agent.t.db;
    let stamp = |build: Uuid, at| {
        app_builds::Entity::update_many()
            .col_expr(app_builds::Column::CreatedAt, Expr::value(at))
            .filter(app_builds::Column::Id.eq(build))
    };
    let now = Utc::now().fixed_offset();
    stamp(newer, now - Duration::days(2))
        .exec(db)
        .await
        .expect("age");
    stamp(stg, now - Duration::days(1))
        .exec(db)
        .await
        .expect("age");

    let mv = move_to(&agent, BuildRef::Newest);
    race(db, app_id, mv, |txn| async move {
        let ahead = now + Duration::hours(1);
        stamp(newer, ahead).exec(&txn).await.expect("newest");
        txn
    })
    .await
    .expect("the move follows the newest");

    assert_eq!(pointers(&agent).await.0, Some(newer), "the newest build");
}
