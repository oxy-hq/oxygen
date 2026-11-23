//! An org's workspace list never offers the local-mode nil workspace.
//!
//! Was `the_workspace_list_never_offers_the_local_mode_nil_workspace` in
//! `oxy-api-tenancy`'s `org_default_workspace`, which created its org through
//! that crate's staff `create_org` handler. `list_workspaces` is this crate's
//! since 2026-11-16, and S3 forbids either surface depending on the other, dev
//! dependencies included, so the org, its owner and the membership are written
//! as rows here instead. What the case asserts is unchanged.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-workspaces --test integration -E 'test(workspace_list)'`

use axum::Json;
use axum::extract::State;
use entity::org_members::{self, OrgRole};
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, users, workspaces};
use oxy_api_workspaces::workspaces::list_workspaces;
use oxy_app::surface::bare_app_state;
use oxy_server_authz::org_context::{OrgContext, OrgContextExtractor};
use sea_orm::{ActiveModelTrait, ActiveValue};
use uuid::Uuid;

use crate::common::test_db;

/// A database a `--local` run once touched carries a nil "Local" row in its
/// org; listed, the post-login dispatcher picks it and the workspace shell
/// spins on `/meta` 404s forever.
#[tokio::test]
async fn the_workspace_list_never_offers_the_local_mode_nil_workspace() {
    let db = test_db().await;

    let user_id = Uuid::new_v4();
    let user = users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("owner-{user_id}@example.com"))),
        name: ActiveValue::Set("Owner".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        status: ActiveValue::Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the owner");

    let org_id = Uuid::new_v4();
    let org = organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Default Workspace Co".into()),
        slug: ActiveValue::Set(format!("dw-{}", org_id.simple())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the org");

    let membership = org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user.id),
        role: ActiveValue::Set(OrgRole::Owner),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the owner's membership");

    let now = chrono::Utc::now();
    workspaces::ActiveModel {
        id: ActiveValue::Set(Uuid::nil()),
        name: ActiveValue::Set("Local".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        created_at: ActiveValue::Set(now.into()),
        updated_at: ActiveValue::Set(now.into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed the row a --local run leaves behind");

    let ctx = OrgContext {
        org,
        membership,
        is_global_override: false,
    };

    let Json(listed) = list_workspaces(
        OrgContextExtractor(ctx),
        oxy_server_authz::Caller::without_credential(user.id, ""),
        State(bare_app_state()),
    )
    .await
    .expect("list workspaces");
    let ids: Vec<Uuid> = listed.iter().map(|w| w.id).collect();
    assert!(
        ids.is_empty(),
        "the nil workspace is never listed, so this org lists none: {ids:?}"
    );
}
