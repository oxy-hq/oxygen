//! The per-app admin role under a token's ceiling (design §3.2, §4.4).
//!
//! An `app_members` admin row makes a non-officer the app's administrator. It
//! is an admin's authority, so a token carries it only where its ceiling over
//! the workspace the app was published from reaches admin. Below that the
//! caller reads as the app's `member`: the role `ctx.user.appRole` reports and
//! `require_app_admin` (the app's logs and symbolicated stacks) refuses.
//!
//! Both read `resolve_app_role`, driven here with the caller the custom-app
//! path builds for each credential.

use entity::app_members;
use entity::org_members::OrgRole;
use oxy_app::server::api::custom_apps_auth::resolve_app_role;
use oxy_app::server::authz::Caller;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::RoleCeiling;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use super::stack::{Reach, custom_app_caller, join_org, mint, published_app, workspace_grant};
use super::{endpoint_key, fixture, legacy_key, seed_workspace_in};

async fn make_app_admin(db: &DatabaseConnection, app_id: Uuid, user_id: Uuid) {
    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(app_members::ROLE_ADMIN.to_string()),
        created_at: ActiveValue::Set(chrono::Utc::now().into()),
        created_by: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed the app-admin row");
}

#[tokio::test]
async fn a_per_app_admin_is_an_app_admin_only_at_an_admin_ceiling() {
    // A plain org member — no officer term to pass on — who administers one app.
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Member).await;
    let ws = fx.workspace_id;
    let app = published_app(&fx.db, fx.org_id, ws).await;
    make_app_admin(&fx.db, app.id, fx.user.id).await;

    let session = Caller::from_user(&AuthenticatedUser::from(fx.user.clone()));
    assert_eq!(
        resolve_app_role(&fx.db, &session, &app).await.unwrap(),
        Some("admin"),
        "the session of an app admin"
    );

    for (ceiling, want) in [
        (RoleCeiling::Viewer, "member"),
        (RoleCeiling::Member, "member"),
        (RoleCeiling::Admin, "admin"),
        (RoleCeiling::Owner, "admin"),
    ] {
        let grants = vec![workspace_grant(fx.org_id, ws, ceiling)];
        let (_, secret) = mint(&fx.db, fx.user.id, Reach::granted(grants)).await;
        let caller = custom_app_caller(&fx, &secret).await;
        assert_eq!(
            resolve_app_role(&fx.db, &caller, &app).await.unwrap(),
            Some(want),
            "a {ceiling:?}-ceiling token of an app admin"
        );
    }

    // An admin grant on a sibling workspace says nothing about this app.
    let sibling = seed_workspace_in(&fx.db, fx.org_id).await;
    let grants = vec![
        workspace_grant(fx.org_id, ws, RoleCeiling::Viewer),
        workspace_grant(fx.org_id, sibling, RoleCeiling::Admin),
    ];
    let (_, secret) = mint(&fx.db, fx.user.id, Reach::granted(grants)).await;
    let caller = custom_app_caller(&fx, &secret).await;
    assert_eq!(
        resolve_app_role(&fx.db, &caller, &app).await.unwrap(),
        Some("member"),
        "viewer on the app's workspace, admin on another"
    );
}

#[tokio::test]
async fn an_uncapped_credential_of_an_app_admin_is_still_an_app_admin() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Member).await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    make_app_admin(&fx.db, app.id, fx.user.id).await;

    let (_, all_access) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (_, seeded) = legacy_key(&fx, None).await;
    let (_, minted) = endpoint_key(&fx, None).await;
    for (what, secret) in [
        ("an all-access token", all_access),
        ("a legacy key", seeded),
        ("a key the legacy endpoint minted", minted),
    ] {
        let caller = custom_app_caller(&fx, &secret).await;
        assert_eq!(
            resolve_app_role(&fx.db, &caller, &app).await.unwrap(),
            Some("admin"),
            "{what}"
        );
    }
}
