//! Who reaches the sandbox management routes (`sandbox_routes` drives what
//! they do): each handler's own refusals — a caller without non-production
//! reach, a publish token — and the staff console's layers in front of them:
//! a caller with no platform standing, an operator whose grant reaches
//! another org, and that production mounts the routes inside those layers.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use entity::users::UserStatus;
use oxy_app::server::api::custom_apps_sandboxes::SandboxError;
use oxy_app::server::api::custom_apps_sandboxes::handlers::{self, CreateEnvironmentRequest};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app_environments::seed_sandbox;
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_routes::{published_app, sandbox_rows, send, staffed_app};

fn caller(id: Uuid, email: &str) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id,
        email: Some(email.to_string()),
        name: "Sandboxes".to_string(),
        picture: None,
        status: UserStatus::Active,
    })
}

async fn refusal(result: Result<impl IntoResponse, SandboxError>) -> (StatusCode, String) {
    let error = result.map(|_| ()).expect_err("refused");
    (error.status(), error.code().to_string())
}

/// Past the console's own guards, each handler still decides for itself:
/// a caller oxy-authz does not let open the app's non-production
/// environments is `403 non_production_refused`, and a publish token is
/// `403 publish_token_refused` whoever minted it. Nothing is created.
#[tokio::test]
async fn a_caller_without_reach_and_a_publish_token_are_refused_by_every_handler() {
    let t = seeded_tenant().await;
    let app = staffed_app(&t).await;
    seed_sandbox(&t.db, app, "dev-a1", t.guest_id).await;
    let request = |name: &str| {
        Json(CreateEnvironmentRequest {
            name: name.to_string(),
        })
    };
    let named = || Path((app, "dev-a1".to_string()));

    let tenant = || caller(Uuid::new_v4(), "tenant-admin@customer.example");
    let expected = (StatusCode::FORBIDDEN, "non_production_refused".to_string());
    assert_eq!(
        refusal(handlers::list(tenant(), None, Path(app)).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::create(tenant(), None, Path(app), request("dev-b2")).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::show(tenant(), None, named()).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::delete(tenant(), None, named()).await).await,
        expected
    );

    let staff = || caller(t.guest_id, LOCAL_GUEST_EMAIL);
    let token = || {
        Some(Extension(AppPublishTokenAuth {
            token_id: Uuid::new_v4(),
            app_id: Some(app),
            machine_identity: None,
        }))
    };
    let expected = (StatusCode::FORBIDDEN, "publish_token_refused".to_string());
    assert_eq!(
        refusal(handlers::list(staff(), token(), Path(app)).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::create(staff(), token(), Path(app), request("dev-b2")).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::show(staff(), token(), named()).await).await,
        expected
    );
    assert_eq!(
        refusal(handlers::delete(staff(), token(), named()).await).await,
        expected
    );

    assert_eq!(
        sandbox_rows(&t.db, app).await,
        vec![("dev-a1".to_string(), false, false)],
        "nothing was created or marked"
    );
    // Control: the same staff caller without the token is let through.
    let _shown = handlers::show(staff(), None, named()).await.expect("staff");
}

/// Without platform standing the console's own layers answer first, with
/// no body — the handler is never reached.
#[tokio::test]
async fn the_console_guards_refuse_a_caller_who_is_not_staff() {
    let t = seeded_tenant().await;
    let app = published_app(&t).await;
    let (status, body) = send("GET", &format!("{app}/environments"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, Value::Null, "a router layer's refusal has no body");
    let (status, _) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(sandbox_rows(&t.db, app).await.is_empty());
}

/// Platform standing for the guest that reaches `orgs` and no other: an
/// App Operator's grant, scoped.
async fn scope_guest_to(db: &DatabaseConnection, orgs: &[Uuid]) {
    use sea_orm::{ActiveModelTrait, ActiveValue};
    let grant = Uuid::new_v4();
    entity::app_admins::ActiveModel {
        id: ActiveValue::Set(grant),
        email: ActiveValue::Set(LOCAL_GUEST_EMAIL.to_string()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set("app_operator".to_string()),
        scope_all: ActiveValue::Set(false),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed the grant");
    for org in orgs {
        entity::app_admin_scope_orgs::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_admin_id: ActiveValue::Set(grant),
            org_id: ActiveValue::Set(*org),
            created_at: ActiveValue::NotSet,
            created_by: ActiveValue::Set(None),
        }
        .insert(db)
        .await
        .expect("seed the grant's scope");
    }
}

/// An App Operator whose grant reaches another org is told the app is not
/// there — the console layer's bare `404`, on every route — and creates
/// nothing. Not `403`: an out-of-scope caller must not learn which apps
/// exist.
#[tokio::test]
async fn an_operator_scoped_to_another_org_finds_no_app() {
    let t = seeded_tenant().await;
    // The grant first: platform standing is cached per email, and the publish
    // below asks for it.
    let elsewhere = crate::custom_app_functions_fixture::throwaway_org(&t).await;
    scope_guest_to(&t.db, &[elsewhere.org_id]).await;
    let app = published_app(&t).await;
    seed_sandbox(&t.db, app, "dev-a1", t.guest_id).await;

    for (method, path, body) in [
        ("GET", format!("{app}/environments"), None),
        (
            "POST",
            format!("{app}/environments"),
            Some(json!({ "name": "dev-b2" })),
        ),
        ("GET", format!("{app}/environments/dev-a1"), None),
        ("DELETE", format!("{app}/environments/dev-a1"), None),
    ] {
        let (status, body) = send(method, &path, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {body}");
        assert_eq!(
            body,
            Value::Null,
            "{method} {path}: a layer's refusal has no body"
        );
    }
    assert_eq!(
        sandbox_rows(&t.db, app).await,
        vec![("dev-a1".to_string(), false, false)],
        "nothing was created or marked"
    );
}

/// The same grant, reaching the app's own org, is let through all four
/// layers and the handler's own check: the control for the test above.
#[tokio::test]
async fn an_operator_scoped_to_the_apps_org_uses_its_sandboxes() {
    let t = seeded_tenant().await;
    // The grant first, as above.
    scope_guest_to(&t.db, &[t.org_id]).await;
    let app = published_app(&t).await;
    let (status, created) = send(
        "POST",
        &format!("{app}/environments"),
        Some(json!({ "name": "dev-a1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, listed) = send("GET", &format!("{app}/environments"), None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
}

/// Production mounts the two paths in the `/customer-apps` nest of
/// `router::global`, **above** the four layers [`console`] copies — axum
/// applies a `.layer` only to routes registered before it — and names the
/// app id `id`, which `enforce_app_scope` reads by name.
#[test]
fn the_routes_are_mounted_inside_the_console_guards() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/router/global.rs"),
    )
    .expect("read router/global.rs");
    let nest = source
        .find("\"/customer-apps\",")
        .expect("router/global.rs mounts the /customer-apps nest");
    // Everything is looked for inside the nest: the same guards also wrap
    // other trees earlier in the file.
    let position = |needle: &str| {
        nest + source[nest..]
            .find(needle)
            .unwrap_or_else(|| panic!("the /customer-apps nest no longer contains {needle:?}"))
    };
    let routes = [
        position("\"/{id}/environments\","),
        position("\"/{id}/environments/{name}\","),
    ];
    let layers = [
        position(".layer(middleware::from_fn(admin::assume::block_admin_while_acting))"),
        position(".layer(middleware::from_fn(app_scope_guard::enforce_app_scope))"),
        position("crate::server::authz::Action::PlatformApps,"),
        position("oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware,"),
    ];
    for route in routes {
        for layer in layers {
            assert!(
                route < layer,
                "a route registered after a layer is not covered by it"
            );
        }
    }
    for handler in ["list", "create", "show", "delete"] {
        position(&format!("custom_apps_sandboxes::handlers::{handler})"));
    }
}
