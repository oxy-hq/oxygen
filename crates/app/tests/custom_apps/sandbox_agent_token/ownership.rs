//! What a sandbox agent token owns, decided on the row each write locks: the
//! per-token limit counts sandboxes still being torn down, and a delete whose
//! row changed creator after the request was admitted touches nothing.

use oxy_app::server::api::custom_apps_sandboxes::{
    MAX_SANDBOXES_PER_TOKEN, SandboxError, TeardownReason, ops,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::fixture::{app_model, token_actor};
use crate::app_environments::seed_sandbox;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_routes::{sandbox_rows, staffed_app};

fn sandbox(name: &str) -> AppEnvironment {
    AppEnvironment::parse(name).expect("a sandbox name")
}

/// Three created, one deleted with its teardown still queued: the fourth
/// create is `token_sandbox_limit`, because the one being torn down still
/// counts. Otherwise a token could create, delete and create again faster
/// than the teardowns run.
#[tokio::test]
async fn a_sandbox_being_torn_down_still_counts_toward_the_token_limit() {
    let t = seeded_tenant().await;
    let app = app_model(&t.db, staffed_app(&t).await).await;
    let actor = token_actor(&t, Uuid::new_v4(), &app);
    assert_eq!(MAX_SANDBOXES_PER_TOKEN, 3);
    for name in ["dev-t1", "dev-t2", "dev-t3"] {
        ops::create(&t.db, &app, &sandbox(name), &actor)
            .await
            .unwrap_or_else(|e| panic!("create {name}: {e}"));
    }
    ops::begin_delete(
        &t.db,
        &app,
        &sandbox("dev-t1"),
        Some(&actor),
        TeardownReason::Deleted,
    )
    .await
    .expect("the token deletes its own sandbox");
    let deleting = sandbox_rows(&t.db, app.id).await;
    assert!(
        deleting
            .iter()
            .any(|(name, _, deleting)| name == "dev-t1" && *deleting),
        "the teardown is still pending: {deleting:?}"
    );

    let fourth = ops::create(&t.db, &app, &sandbox("dev-t4"), &actor).await;
    assert_eq!(
        fourth.err(),
        Some(SandboxError::TokenLimit(MAX_SANDBOXES_PER_TOKEN))
    );
}

/// The race: the token was admitted against its own `dev-a`, which then
/// finished its teardown, and a colleague created another `dev-a`. The delete
/// is decided on the row it locks, which is the colleague's: `NotFound`, and
/// the colleague's sandbox is left exactly as it was.
#[tokio::test]
async fn a_token_delete_leaves_a_sandbox_someone_else_created() {
    let t = seeded_tenant().await;
    let app = app_model(&t.db, staffed_app(&t).await).await;
    seed_sandbox(&t.db, app.id, "dev-a", t.guest_id).await;
    let actor = token_actor(&t, Uuid::new_v4(), &app);

    let refused = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("dev-a"),
        Some(&actor),
        TeardownReason::Deleted,
    )
    .await;

    assert_eq!(refused.err(), Some(SandboxError::NotFound("dev-a".into())));
    assert_eq!(
        sandbox_rows(&t.db, app.id).await,
        vec![("dev-a".to_string(), false, false)],
        "the colleague's sandbox is not marked deleting"
    );
}

/// The same delete by another token is refused too; by the token that
/// created it, it goes through.
#[tokio::test]
async fn only_the_creating_token_deletes_its_sandbox() {
    let t = seeded_tenant().await;
    let app = app_model(&t.db, staffed_app(&t).await).await;
    let (owner, other) = (
        token_actor(&t, Uuid::new_v4(), &app),
        token_actor(&t, Uuid::new_v4(), &app),
    );
    ops::create(&t.db, &app, &sandbox("dev-own"), &owner)
        .await
        .expect("create");

    let by_other = ops::begin_delete(
        &t.db,
        &app,
        &sandbox("dev-own"),
        Some(&other),
        TeardownReason::Deleted,
    )
    .await;
    assert_eq!(
        by_other.err(),
        Some(SandboxError::NotFound("dev-own".into()))
    );
    ops::begin_delete(
        &t.db,
        &app,
        &sandbox("dev-own"),
        Some(&owner),
        TeardownReason::Deleted,
    )
    .await
    .expect("the creating token deletes it");
}
