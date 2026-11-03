//! Pinning a custom-app environment's semantic model to a workspace branch
//! (`oxy_app::server::compile_reconcile::env_pins`).
//!
//! The writer half of the pin the reader already understands: `pin_to_branch`
//! stores a ready revision or the commit still being waited on,
//! `move_after_compile` is the one statement that moves every waiting
//! environment when that commit compiles, and a planes write that releases or
//! repoints the environment (`planes::replace`, mirrored here by
//! [`replace_planes`]) is what stops a late compile from pinning it.
//!
//! What a live database shows that a unit test cannot: the move matches on the
//! commit and the workspace together, a cleared or repointed pin is not moved,
//! two environments on one branch both move, a repin to an uncompiled branch
//! keeps the environment on the revision it was serving, and a pin lands only
//! on a row that still names its branch. The branch
//! lookup is a `wiremock` GitHub, from [`super::compile_request::fixture`].
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(env_pins)'`

use chrono::Utc;
use entity::{app_environments, apps, organizations, workspaces};
use oxy_app::server::compile_reconcile::env_pinner::EnvPinner;
use oxy_app::server::compile_reconcile::env_pins::{
    PinOutcome, branch_exists, move_after_compile, pin_to_branch, settle_pending_pin,
};
use oxy_app::server::compile_request::Refusal;
use oxy_app_core::semantic_pin_seam::SemanticPinner;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use uuid::Uuid;
use wiremock::ResponseTemplate;

use crate::compile_request::fixture::{BRANCH, Fx, HEAD};

mod push;

const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

/// An org, and an app of `workspace` in it with a `staging` environment.
async fn seed_app(db: &DatabaseConnection, org: Uuid, workspace: Uuid) -> Uuid {
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(org),
        project_id: ActiveValue::Set(workspace),
        slug: ActiveValue::Set(format!("app-{}", app_id.simple())),
        name: ActiveValue::Set("Store Ops".into()),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("git@example.com:acme/store-ops.git".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("org".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO app_environments (app_id, name, kind) VALUES ($1, 'staging', 'staging')",
        [app_id.into()],
    ))
    .await
    .expect("seed environment");
    app_id
}

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("pins-org".into()),
        slug: ActiveValue::Set(format!("pins-{}", id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

/// A second workspace, so a move can be shown not to cross workspaces.
async fn other_workspace(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(Some(org)),
        name: ActiveValue::Set("other".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed other workspace");
    id
}

/// The environment's two pin columns: `(semantic_revision_id, semantic_git_sha)`.
async fn pin_of(db: &DatabaseConnection, app: Uuid) -> (Option<Uuid>, Option<String>) {
    let row = app_environments::Entity::find_by_id((app, "staging".to_string()))
        .one(db)
        .await
        .expect("read environment")
        .expect("environment exists");
    (row.semantic_revision_id, row.semantic_git_sha)
}

/// Make the environment wait on `sha`, as a repin to an uncompiled branch does.
async fn wait_on(db: &DatabaseConnection, app: Uuid, sha: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments SET semantic_git_sha = $2 WHERE app_id = $1",
        [app.into(), sha.into()],
    ))
    .await
    .expect("set the waiting sha");
}

/// Point the environment's `semantic` plane at `branch`, as a planes write
/// does before its pin is set.
async fn name_branch(db: &DatabaseConnection, app: Uuid, branch: &str) {
    replace_planes(
        db,
        app,
        serde_json::json!({ "semantic": format!("branch:{branch}") }),
    )
    .await;
}

/// What `planes::replace` does to the row, in one statement: store `planes` and
/// drop the previous pin (both columns), so a pin is never served under a row
/// that names another branch.
async fn replace_planes(db: &DatabaseConnection, app: Uuid, planes: serde_json::Value) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE app_environments \
         SET planes = $2::jsonb, semantic_revision_id = NULL, semantic_git_sha = NULL \
         WHERE app_id = $1 AND name = 'staging'",
        [app.into(), planes.to_string().into()],
    ))
    .await
    .expect("replace planes");
}

#[tokio::test]
async fn move_after_compile_moves_matching_sha_only() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let waiting = seed_app(&fx.db, org, fx.ws).await;
    let elsewhere_sha = seed_app(&fx.db, org, fx.ws).await;
    let elsewhere_ws = seed_app(&fx.db, org, other_workspace(&fx.db, org).await).await;
    wait_on(&fx.db, waiting, NEW).await;
    wait_on(&fx.db, elsewhere_sha, OLD).await;
    wait_on(&fx.db, elsewhere_ws, NEW).await;
    let revision = Uuid::new_v4();

    let moved = move_after_compile(&fx.db, fx.ws, NEW, revision)
        .await
        .expect("move");

    assert_eq!(moved, 1, "only the environment waiting on NEW here");
    assert_eq!(
        pin_of(&fx.db, waiting).await,
        (Some(revision), Some(NEW.into()))
    );
    assert_eq!(
        pin_of(&fx.db, elsewhere_sha).await,
        (None, Some(OLD.into()))
    );
    assert_eq!(
        pin_of(&fx.db, elsewhere_ws).await,
        (None, Some(NEW.into())),
        "another workspace's compile of the same sha is not this one's"
    );
}

#[tokio::test]
async fn cleared_pin_is_not_moved_by_late_compile() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    name_branch(&fx.db, app, BRANCH).await;
    wait_on(&fx.db, app, NEW).await;

    // The owner leaves the branch: the planes write clears the pin.
    replace_planes(&fx.db, app, serde_json::json!({})).await;
    let moved = move_after_compile(&fx.db, fx.ws, NEW, Uuid::new_v4())
        .await
        .expect("move");

    assert_eq!(moved, 0);
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
}

#[tokio::test]
async fn two_envs_on_one_branch_both_move() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let first = seed_app(&fx.db, org, fx.ws).await;
    let second = seed_app(&fx.db, org, fx.ws).await;
    wait_on(&fx.db, first, NEW).await;
    wait_on(&fx.db, second, NEW).await;
    let revision = Uuid::new_v4();

    let moved = move_after_compile(&fx.db, fx.ws, NEW, revision)
        .await
        .expect("move");

    assert_eq!(moved, 2);
    for app in [first, second] {
        assert_eq!(
            pin_of(&fx.db, app).await,
            (Some(revision), Some(NEW.into()))
        );
    }
}

#[tokio::test]
async fn pinning_a_compiled_branch_sets_both_columns() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    name_branch(&fx.db, app, BRANCH).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let revision = fx.ready_revision(HEAD, "staging").await;

    let outcome = pin_to_branch(&fx.db, &fx.workspace().await, app, "staging", BRANCH)
        .await
        .expect("pin");

    assert_eq!(
        outcome,
        PinOutcome::Pinned {
            revision_id: revision,
            git_sha: HEAD.into()
        }
    );
    assert_eq!(
        pin_of(&fx.db, app).await,
        (Some(revision), Some(HEAD.into()))
    );
}

#[tokio::test]
async fn repinning_to_an_uncompiled_branch_keeps_serving_the_old_revision() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    let old = Uuid::new_v4();
    name_branch(&fx.db, app, BRANCH).await;
    wait_on(&fx.db, app, OLD).await;
    move_after_compile(&fx.db, fx.ws, OLD, old)
        .await
        .expect("pin old");
    fx.branch_is_at(BRANCH, HEAD).await;

    let outcome = pin_to_branch(&fx.db, &fx.workspace().await, app, "staging", BRANCH)
        .await
        .expect("pin");

    assert_eq!(
        outcome,
        PinOutcome::Compiling {
            git_sha: HEAD.into()
        }
    );
    assert_eq!(
        pin_of(&fx.db, app).await,
        (Some(old), Some(HEAD.into())),
        "the old model keeps serving; only the waiting sha moved"
    );
    let landed = Uuid::new_v4();
    move_after_compile(&fx.db, fx.ws, HEAD, landed)
        .await
        .expect("land");
    assert_eq!(pin_of(&fx.db, app).await, (Some(landed), Some(HEAD.into())));
}

#[tokio::test]
async fn production_takes_no_pin() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    fx.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO app_environments (app_id, name, kind) VALUES ($1, 'production', 'production')",
            [app.into()],
        ))
        .await
        .expect("seed production");
    fx.branch_is_at(BRANCH, HEAD).await;

    let refused = pin_to_branch(&fx.db, &fx.workspace().await, app, "production", BRANCH).await;

    assert!(matches!(refused, Err(Refusal::Conflict(_))), "{refused:?}");
}

#[tokio::test]
async fn an_unknown_branch_is_refused_by_name_and_nothing_is_written() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    fx.github_answers("nope", ResponseTemplate::new(404)).await;

    let workspace = fx.workspace().await;
    let refused = branch_exists(&workspace, "nope").await;
    let pinned = pin_to_branch(&fx.db, &workspace, app, "staging", "nope").await;

    assert!(refused.is_err(), "{refused:?}");
    assert!(pinned.is_err(), "{pinned:?}");
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
}

/// The port the planes routes call (`EnvPinner`) over a live database: an
/// unknown branch is refused as a bad branch (the planes write answers it
/// `400 invalid_planes`) and writes nothing; a compiled branch pins both
/// columns; an unknown workspace is its own refusal.
#[tokio::test]
async fn the_seam_pinner_checks_pins_and_clears() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    name_branch(&fx.db, app, BRANCH).await;
    fx.github_answers("nope", ResponseTemplate::new(404)).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    let revision = fx.ready_revision(HEAD, "staging").await;

    let refused = EnvPinner
        .check_branch(fx.ws, "nope")
        .await
        .expect_err("unknown");
    assert!(refused.is_bad_branch(), "{refused:?}");
    assert_eq!(refused.status, axum::http::StatusCode::NOT_FOUND);
    assert!(refused.message.contains("nope"), "{refused}");
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
    EnvPinner.check_branch(fx.ws, BRANCH).await.expect("known");

    let missing = EnvPinner
        .check_branch(Uuid::new_v4(), BRANCH)
        .await
        .expect_err("no such workspace");
    assert!(!missing.is_bad_branch(), "{missing:?}");

    EnvPinner
        .pin(fx.ws, app, "staging", BRANCH)
        .await
        .expect("pin");
    assert_eq!(
        pin_of(&fx.db, app).await,
        (Some(revision), Some(HEAD.into()))
    );
}

#[tokio::test]
async fn compile_finished_before_pin_write_still_pins() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    // The sha is stored, then the revision is found: the order `pin_to_branch`
    // runs them in after a compile that finished before its write.
    wait_on(&fx.db, app, NEW).await;
    assert_eq!(
        settle_pending_pin(&fx.db, fx.ws, NEW)
            .await
            .expect("settle"),
        None,
        "nothing compiled yet: the worker's move will land it"
    );
    assert_eq!(pin_of(&fx.db, app).await, (None, Some(NEW.into())));
    let revision = fx.ready_revision(NEW, "staging").await;

    let settled = settle_pending_pin(&fx.db, fx.ws, NEW)
        .await
        .expect("settle");

    assert_eq!(settled, Some(revision));
    assert_eq!(
        pin_of(&fx.db, app).await,
        (Some(revision), Some(NEW.into()))
    );
}

#[tokio::test]
async fn an_environment_being_deleted_takes_no_pin_and_no_move() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    name_branch(&fx.db, app, BRANCH).await;
    wait_on(&fx.db, app, NEW).await;
    fx.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments SET deleting_at = now() WHERE app_id = $1",
            [app.into()],
        ))
        .await
        .expect("mark deleting");

    let moved = move_after_compile(&fx.db, fx.ws, NEW, Uuid::new_v4())
        .await
        .expect("move");
    fx.branch_is_at(BRANCH, HEAD).await;
    let refused = pin_to_branch(&fx.db, &fx.workspace().await, app, "staging", BRANCH).await;

    assert_eq!(moved, 0);
    assert!(matches!(refused, Err(Refusal::Conflict(_))), "{refused:?}");
    assert_eq!(pin_of(&fx.db, app).await, (None, Some(NEW.into())));
}

/// Two planes writes race: W1 stores `branch:feat/x`, W2 stores `branch:other`
/// (and clears the pin), then W1's pin arrives. The row names `other` now, so
/// W1's pin writes nothing and is not an error; `feat/x`'s revision is never
/// served under a row that names another branch.
#[tokio::test]
async fn a_pin_for_a_branch_the_row_has_since_left_writes_nothing() {
    let fx = Fx::new(None).await;
    let org = seed_org(&fx.db).await;
    let app = seed_app(&fx.db, org, fx.ws).await;
    fx.branch_is_at(BRANCH, HEAD).await;
    fx.ready_revision(HEAD, "staging").await;
    name_branch(&fx.db, app, BRANCH).await;
    name_branch(&fx.db, app, "other").await;

    let outcome = pin_to_branch(&fx.db, &fx.workspace().await, app, "staging", BRANCH)
        .await
        .expect("a superseded pin is not an error");

    assert_eq!(
        outcome,
        PinOutcome::Superseded {
            git_sha: HEAD.into()
        }
    );
    assert_eq!(pin_of(&fx.db, app).await, (None, None));
}
