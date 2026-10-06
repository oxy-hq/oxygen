//! What a stored sandbox agent token is, to the request path: where it
//! authenticates, what admission makes of its row, and the facts the loader
//! gives a request on it (sandbox agent credential design §3.2, §4, §7.2).
//!
//! **Where it authenticates.** `/api` admits it behind the route allow-list
//! (`app_grant_scope`), and `/fn` and `/logs` behind their own checks. Every
//! other entry point passes `SandboxAgent::Refuse` and answers 401. What a
//! request on the token carries is also read from the store, the function the
//! dispatch calls.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::org_members::OrgRole;
use entity::users::UserStatus;
use entity::{api_token_grants, api_tokens, app_admins, apps, users};
use oxy_app::server::authz::loader;
use oxy_app_core::audit::RequestActor;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::token::{StoredKind, leak};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{Action, EnvFacet, Resource, RoleCeiling, SandboxApp, Scope, TokenGrant, allows};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use super::sandbox_agent::{admitted, grants_of, minted, staff_with_app, token_caller};
use super::stack::{Reach, flat_api, get_as, join_org, mint, published_app};
use super::{call, external_surface, pat_row, probe_as, seed_workspace_in};

#[tokio::test]
async fn the_token_authenticates_on_api_only_and_reaches_the_loop_only() {
    let (fx, app) = staff_with_app().await;
    let (id, secret) = minted(&fx, &[app.id]).await;
    let bearer = format!("Bearer {secret}");

    // `/api` admits it, in either header, and A1 answers.
    let (status, described) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{described}");
    assert_eq!(described["id"], id.to_string());
    assert_eq!(described["kind"], "sandbox_agent");
    let (status, _) = call(
        flat_api(),
        "GET",
        "/auth/token",
        &[("x-api-key", &secret)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Everything else on `/api` is 404, the route allow-list's answer — its
    // own management routes and its minter's console included.
    for uri in [
        "/user/tokens",
        "/user/token-options",
        "/orgs",
        "/customer-apps",
        "/admin/orgs-meta",
        "/assume",
    ] {
        assert_eq!(get_as(&secret, uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    let workspace_route = format!("/{}/probe", fx.workspace_id);
    let (status, _) = call(
        super::api_surface(),
        "GET",
        &workspace_route,
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a workspace route");

    // `?api_key=`: never in a URL.
    let query = format!("?api_key={secret}");
    let (status, _) = probe_as(&fx, super::api_surface(), &query, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // `/external/api`.
    let (status, _) = probe_as(&fx, external_surface(), "", &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // An authenticator built to refuse it: custom-app serving and gates,
    // `GET /api/user`, kiosk enrol, `/errors`, `/debug`, `/health`.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", bearer.parse().unwrap());
    assert!(
        BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
            .authenticate_with_credential(&headers)
            .await
            .is_err()
    );
    // It never falls through to a session either: with a valid cookie beside
    // it, the token still decides the request.
    let (status, _) = call(
        flat_api(),
        "GET",
        "/user/tokens",
        &[("authorization", &bearer), ("cookie", &fx.cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A2: it ends itself, and is refused from then on.
    let (status, _) = call(
        flat_api(),
        "DELETE",
        "/auth/token",
        &[("authorization", &bearer)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(admitted(&fx, &secret).await.is_err());
}

#[tokio::test]
async fn admission_gives_each_app_a_workspace_grant_at_admin_where_it_lives_now() {
    let (fx, app) = staff_with_app().await;
    let sibling = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let (id, secret) = minted(&fx, &[app.id, sibling.id]).await;

    let credential = admitted(&fx, &secret).await.expect("admitted");
    assert_eq!(credential.kind, StoredKind::SandboxAgent);
    assert_eq!(credential.token_id, id);
    assert_eq!(credential.principal_user_id, fx.user.id);
    assert!(credential.sandboxes_app(app.id) && credential.sandboxes_app(sibling.id));
    // Two apps of one workspace: one grant between them.
    assert_eq!(
        credential.grants,
        vec![TokenGrant {
            org_id: fx.org_id,
            workspace_id: Some(fx.workspace_id),
            ceiling: RoleCeiling::Admin,
        }]
    );

    // An app re-pointed at another workspace takes its grant with it.
    let moved_to = seed_workspace_in(&fx.db, fx.org_id).await;
    let mut active: apps::ActiveModel = sibling.clone().into();
    active.project_id = ActiveValue::Set(moved_to);
    active.update(&fx.db).await.expect("re-point the app");
    let credential = admitted(&fx, &secret).await.expect("admitted");
    let mut workspaces: Vec<Option<Uuid>> =
        credential.grants.iter().map(|g| g.workspace_id).collect();
    workspaces.sort();
    let mut expected = vec![Some(fx.workspace_id), Some(moved_to)];
    expected.sort();
    assert_eq!(workspaces, expected);

    // A revoked grant row is excluded on the next read; the other stands.
    let grant = grants_of(&fx.db, id)
        .await
        .into_iter()
        .find(|g| g.app_id == Some(sibling.id))
        .expect("the sibling's grant");
    let mut active: api_token_grants::ActiveModel = grant.into();
    active.revoked_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    active.update(&fx.db).await.expect("revoke the grant");
    let credential = admitted(&fx, &secret).await.expect("admitted");
    assert!(credential.sandboxes_app(app.id) && !credential.sandboxes_app(sibling.id));
    assert_eq!(credential.grants.len(), 1);
    assert_eq!(credential.grants[0].workspace_id, Some(fx.workspace_id));

    // A deleted app takes its grant row with it, and the token reaches nothing.
    apps::Entity::delete_by_id(app.id)
        .exec(&fx.db)
        .await
        .expect("delete the app");
    let credential = admitted(&fx, &secret).await.expect("admitted");
    assert!(credential.app_sandbox.is_empty() && credential.grants.is_empty());
    assert!(credential.reach().unwrap().sandbox_agent.is_some());
}

#[tokio::test]
async fn a_dead_token_or_a_dead_minter_is_not_admitted() {
    let (fx, app) = staff_with_app().await;

    // Expired.
    let (id, secret) = minted(&fx, &[app.id]).await;
    let mut active: api_tokens::ActiveModel = pat_row(&fx.db, id).await.into();
    active.expires_at = ActiveValue::Set(Some((Utc::now() - Duration::seconds(1)).fixed_offset()));
    active.update(&fx.db).await.expect("age the token");
    assert!(admitted(&fx, &secret).await.is_err(), "an expired token");

    // Revoked — by a leak report, which revokes this kind like any token.
    let (id, secret) = minted(&fx, &[app.id]).await;
    assert_eq!(leak::classify(&secret), leak::Presented::NewFormat);
    let row = leak::find(&fx.db, &secret).await.unwrap();
    assert_eq!(leak::decide(row.as_ref()), leak::Decision::Revoke);
    let revoked = leak::revoke(&fx.db, id).await.unwrap().expect("revoked");
    assert_eq!(revoked.revoke_reason.as_deref(), Some("leaked"));
    assert!(admitted(&fx, &secret).await.is_err(), "a revoked token");

    // A deactivated minter: the token is refused with them, at once.
    let (_, secret) = minted(&fx, &[app.id]).await;
    assert!(admitted(&fx, &secret).await.is_ok());
    let mut active: users::ActiveModel = fx.user.clone().into();
    active.status = ActiveValue::Set(UserStatus::Deleted);
    active.update(&fx.db).await.expect("deactivate the minter");
    assert!(admitted(&fx, &secret).await.is_err(), "an inactive minter");
}

#[tokio::test]
async fn the_loader_gives_the_token_its_minters_staff_standing_and_no_tenant_standing() {
    let (fx, app) = staff_with_app().await;
    // The minter is also the org's owner: none of that may ride the token.
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let (id, secret) = minted(&fx, &[app.id]).await;

    let caller = token_caller(&fx, &secret).await;
    assert!(caller.is_sandbox_agent());
    let facts = loader::load_principal_facts(&fx.db, &caller)
        .await
        .expect("facts");

    let reach = facts.token.clone().expect("the token's reach");
    assert_eq!(
        reach.grants,
        vec![TokenGrant {
            org_id: fx.org_id,
            workspace_id: Some(fx.workspace_id),
            ceiling: RoleCeiling::Admin,
        }]
    );
    let sandbox = reach.sandbox_agent.expect("the sandbox fact");
    assert_eq!(sandbox.token_id, id);
    assert_eq!(
        sandbox.apps,
        vec![SandboxApp {
            app_id: app.id,
            org_id: fx.org_id
        }]
    );
    assert!(
        facts.member_orgs.is_empty() && facts.admin_orgs.is_empty() && facts.owned_orgs.is_empty(),
        "the owner's membership did not ride the token"
    );
    // The Global Admin's unbounded grant, bounded to the granted app's org.
    assert_eq!(
        facts.platform.as_ref().map(|p| p.scope.clone()),
        Some(Scope::Orgs(vec![fx.org_id]))
    );

    let in_env =
        |facet| Resource::app_environment(app.id, fx.org_id, facet).published_from(fx.workspace_id);
    let own = EnvFacet::Sandbox {
        created_by_token: Some(id),
    };
    assert!(allows(
        &facts,
        Action::AppNonProduction,
        &in_env(EnvFacet::NewSandbox)
    ));
    assert!(allows(&facts, Action::AppNonProduction, &in_env(own)));
    assert!(allows(&facts, Action::AppAdmin, &in_env(own)));
    assert!(!allows(
        &facts,
        Action::AppNonProduction,
        &in_env(EnvFacet::Staging)
    ));
    assert!(!allows(
        &facts,
        Action::AppNonProduction,
        &in_env(EnvFacet::Production)
    ));
    let unfaceted = Resource::app(app.id, fx.org_id).published_from(fx.workspace_id);
    assert!(!allows(&facts, Action::AppNonProduction, &unfaceted));
    // The owner's workspace, through the token: nothing.
    let workspace = Resource::workspace(fx.workspace_id, fx.org_id);
    for action in [
        Action::OrgRead,
        Action::WorkspaceEdit,
        Action::WorkspaceManage,
    ] {
        assert!(!allows(&facts, action, &workspace), "{action:?}");
    }

    // The minter's grant row deleted: the token carries no standing — at
    // once. No cache is dropped here: this kind reads its minter's grant past
    // the 60 s cache, and a request's memo lives on its own caller, so the
    // next request (a caller built again) reads the table as it is now.
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(fx.user.email.clone().unwrap()))
        .exec(&fx.db)
        .await
        .expect("take the grant away");
    let caller = token_caller(&fx, &secret).await;
    let facts = loader::load_principal_facts(&fx.db, &caller)
        .await
        .expect("facts");
    assert!(facts.platform.is_none() && !facts.is_global_owner);
    assert!(!allows(&facts, Action::AppNonProduction, &in_env(own)));
    assert!(!allows(&facts, Action::PlatformApps, &Resource::platform()));
}

#[tokio::test]
async fn the_token_is_told_its_minter_and_its_apps() {
    let (fx, app) = staff_with_app().await;
    let (id, secret) = minted(&fx, &[app.id]).await;
    let credential = admitted(&fx, &secret).await.expect("admitted");

    // `GET /api/auth/token` as the request the token will make once it may
    // authenticate there: the handler, given that request's actor.
    let user = AuthenticatedUser::from(fx.user.clone()).with_credential(Some(credential.clone()));
    let mut actor = RequestActor::session(user);
    actor.credential = Some(credential);
    let described = oxy_app::api::user_tokens::introspect::get_calling_token(actor)
        .await
        .unwrap_or_else(|_| panic!("the calling token is described"));
    let body = serde_json::to_value(&described.0).unwrap();

    assert_eq!(body["id"], json!(id));
    assert_eq!(body["kind"], "sandbox_agent");
    assert_eq!(body["minter"]["user_id"], json!(fx.user.id));
    assert_eq!(body["minter"]["email"], json!(fx.user.email));
    let apps = body["apps"].as_array().expect("apps");
    assert_eq!(apps.len(), 1, "{body}");
    assert_eq!(apps[0]["id"], json!(app.id));
    assert_eq!(apps[0]["slug"], json!(app.slug));
    assert_eq!(apps[0]["name"], "Token Test App");
    assert!(apps[0]["org_slug"].as_str().unwrap().starts_with("acme-"));

    // Every other credential's answer carries neither field.
    let (_, pat) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (status, me) = get_as(&pat, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        me.get("minter").is_none() && me.get("apps").is_none(),
        "{me}"
    );
}
