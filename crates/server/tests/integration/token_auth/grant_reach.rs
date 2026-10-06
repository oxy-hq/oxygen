//! A grant decides what a token reaches (design §3.2, §4.4): the workspace it
//! names and no other, at no more than its ceiling, for as long as its owner
//! still reaches that workspace — on `/api`, on `/external/api` and on the
//! custom-app path alike. Outside the grant the answer is 404.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use oxy_app::server::api::custom_apps_auth::user_can_access_app;
use oxy_app::server::authz::Caller;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::RoleCeiling;

use super::stack::{
    Reach, custom_app_caller, flat_api, get_as, get_in_session, ids, join_org, leave_org,
    make_staff, mint, org_grant, published_app, workspace_api, workspace_external, workspace_grant,
};
use super::{Fixture, call, fixture, seed_org, seed_workspace_in};

/// The fixture's user as an Owner of the fixture's org.
async fn owner_fixture() -> Fixture {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    fx
}

async fn on_api(secret: &str, method: &str, uri: &str) -> StatusCode {
    let bearer = format!("Bearer {secret}");
    call(
        workspace_api(),
        method,
        uri,
        &[("authorization", &bearer)],
        None,
    )
    .await
    .0
}

#[tokio::test]
async fn a_token_granted_workspace_a_gets_404_on_workspace_b() {
    let fx = owner_fixture().await;
    let (a, b) = (fx.workspace_id, seed_workspace_in(&fx.db, fx.org_id).await);
    let (_, secret) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![workspace_grant(fx.org_id, a, RoleCeiling::Owner)]),
    )
    .await;

    // `/api`.
    assert_eq!(
        on_api(&secret, "GET", &format!("/{a}/read")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        on_api(&secret, "GET", &format!("/{b}/read")).await,
        StatusCode::NOT_FOUND,
        "outside the grant a workspace does not exist"
    );

    // `/external/api`, which takes the key from a header only.
    for (workspace, expected) in [(a, StatusCode::NO_CONTENT), (b, StatusCode::NOT_FOUND)] {
        let (status, _) = call(
            workspace_external(),
            "GET",
            &format!("/{workspace}/read"),
            &[("x-api-key", &secret)],
            None,
        )
        .await;
        assert_eq!(status, expected, "/external/api on {workspace}");
    }

    // The 404 is the grant, not the workspace: the same user's session reaches B.
    let (status, _) = call(
        workspace_api(),
        "GET",
        &format!("/{b}/read"),
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_token_reaches_only_the_custom_apps_of_its_granted_workspace() {
    let fx = owner_fixture().await;
    let (a, b) = (fx.workspace_id, seed_workspace_in(&fx.db, fx.org_id).await);
    let app_a = published_app(&fx.db, fx.org_id, a).await;
    let app_b = published_app(&fx.db, fx.org_id, b).await;
    let (_, secret) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![workspace_grant(fx.org_id, a, RoleCeiling::Owner)]),
    )
    .await;

    let caller = custom_app_caller(&fx, &secret).await;
    assert!(
        user_can_access_app(&fx.db, &caller, &app_a).await.unwrap(),
        "the app published from the granted workspace"
    );
    assert!(
        !user_can_access_app(&fx.db, &caller, &app_b).await.unwrap(),
        "an app published from a workspace the token holds no grant on"
    );

    // The owner's session reaches both — again, the refusal is the grant.
    let session = Caller::from_user(&AuthenticatedUser::from(fx.user.clone()));
    assert!(user_can_access_app(&fx.db, &session, &app_b).await.unwrap());
}

#[tokio::test]
async fn a_viewer_ceiling_on_an_admin_reads_but_does_not_write() {
    let fx = owner_fixture().await;
    let ws = fx.workspace_id;
    let token = |ceiling| {
        let (db, user, org) = (fx.db.clone(), fx.user.id, fx.org_id);
        async move {
            mint(
                &db,
                user,
                Reach::granted(vec![workspace_grant(org, ws, ceiling)]),
            )
            .await
            .1
        }
    };

    let viewer = token(RoleCeiling::Viewer).await;
    assert_eq!(
        on_api(&viewer, "GET", &format!("/{ws}/read")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        on_api(&viewer, "POST", &format!("/{ws}/write")).await,
        StatusCode::FORBIDDEN,
        "a viewer ceiling caps an Owner at Viewer"
    );
    assert_eq!(
        on_api(&viewer, "POST", &format!("/{ws}/manage")).await,
        StatusCode::FORBIDDEN
    );

    // One step up writes but does not administer; `owner` is no cap at all.
    let member = token(RoleCeiling::Member).await;
    assert_eq!(
        on_api(&member, "POST", &format!("/{ws}/write")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        on_api(&member, "POST", &format!("/{ws}/manage")).await,
        StatusCode::FORBIDDEN
    );
    let owner = token(RoleCeiling::Owner).await;
    assert_eq!(
        on_api(&owner, "POST", &format!("/{ws}/manage")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_ceiling_never_raises_what_its_owner_holds() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Member).await;
    let ws = fx.workspace_id;
    let (_, secret) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![workspace_grant(fx.org_id, ws, RoleCeiling::Owner)]),
    )
    .await;
    assert_eq!(
        on_api(&secret, "POST", &format!("/{ws}/write")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        on_api(&secret, "POST", &format!("/{ws}/manage")).await,
        StatusCode::FORBIDDEN,
        "an owner ceiling on a Member is still a Member"
    );
}

#[tokio::test]
async fn removing_the_owner_from_the_org_ends_the_grants_reach() {
    let fx = owner_fixture().await;
    let ws = fx.workspace_id;
    let (_, secret) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Owner)]),
    )
    .await;
    assert_eq!(
        on_api(&secret, "GET", &format!("/{ws}/read")).await,
        StatusCode::NO_CONTENT
    );

    leave_org(&fx.db, fx.org_id, fx.user.id).await;

    let status = on_api(&secret, "GET", &format!("/{ws}/read")).await;
    assert!(
        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a grant is a cap on what its owner reaches, not a membership: {status}"
    );
    let bearer = format!("Bearer {secret}");
    let (status, orgs) = call(
        flat_api(),
        "GET",
        "/orgs",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(orgs, serde_json::json!([]), "and it lists no org either");
}

#[tokio::test]
async fn staff_standing_rides_a_token_only_when_it_carries_platform() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let admin = |headers: Vec<(&'static str, String)>| async move {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        call(flat_api(), "GET", "/admin/orgs-meta", &headers, None)
            .await
            .0
    };

    assert_eq!(
        admin(vec![("cookie", fx.cookie.clone())]).await,
        StatusCode::OK,
        "the staff member's own session reaches the console"
    );

    let (_, without) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    assert_eq!(
        admin(vec![("authorization", format!("Bearer {without}"))]).await,
        StatusCode::FORBIDDEN,
        "platform=false: the token is nobody to the admin console"
    );

    let (_, with) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    assert_eq!(
        admin(vec![("authorization", format!("Bearer {with}"))]).await,
        StatusCode::OK
    );
}

/// The grant guard lets every grant-bound token through `/admin/*` and
/// `/customer-apps/*`, and leaves the narrowing to each handler's platform
/// scope. This drives that over HTTP: a staffer whose standing covers every
/// org, holding a token bound to one.
#[tokio::test]
async fn a_grant_bound_staff_token_sees_only_its_grant_org_on_the_consoles() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let other_org = seed_org(&fx.db).await;
    let other_workspace = seed_workspace_in(&fx.db, other_org).await;
    let own_app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let other_app = published_app(&fx.db, other_org, other_workspace).await;
    let app_uri = |app: &entity::apps::Model| format!("/customer-apps/{}", app.id);

    // The control is the staffer's own session, which reaches both orgs.
    let (status, every_org) = get_in_session(&fx, "/admin/orgs-meta").await;
    assert_eq!(status, StatusCode::OK);
    let every_org = ids(&every_org);
    assert!(
        every_org.contains(&fx.org_id) && every_org.contains(&other_org),
        "a session lists every org"
    );
    let (own_in_session, _) = get_in_session(&fx, &app_uri(&own_app)).await;
    let (other_in_session, _) = get_in_session(&fx, &app_uri(&other_app)).await;
    assert_ne!(own_in_session, StatusCode::NOT_FOUND);
    assert_eq!(
        other_in_session, own_in_session,
        "a session reaches an app of either org"
    );

    let reach = Reach::granted(vec![org_grant(fx.org_id, RoleCeiling::Admin)]).with_platform();
    let (_, bound) = mint(&fx.db, fx.user.id, reach).await;

    let (status, listed) = get_as(&bound, "/admin/orgs-meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&listed),
        vec![fx.org_id],
        "the tenant list holds the grant's org and no other"
    );

    let (status, registry) = get_as(&bound, "/customer-apps").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&registry["items"]),
        vec![own_app.id],
        "the app registry holds the grant org's apps and no other"
    );

    assert_eq!(
        get_as(&bound, &app_uri(&own_app)).await.0,
        own_in_session,
        "the grant org's app answers as it does in a session"
    );
    assert_eq!(
        get_as(&bound, &app_uri(&other_app)).await.0,
        StatusCode::NOT_FOUND,
        "an app of another org is not found"
    );
}
