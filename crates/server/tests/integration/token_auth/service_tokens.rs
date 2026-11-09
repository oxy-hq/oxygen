//! Service tokens (`oxysvc_`, `OXY_SERVICE_TOKENS`) over the served stack.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`**, which holds the `ferret`
//! service's token and reads oxy's custom-app verdicts with it.
//!
//! A configured service authenticates with no account, grant or mint: its
//! standing is its entry's role over every org, minus grant management, and a
//! `users` row created at boot backs it. These drive that through the real
//! routers with the token in `x-api-key`, as ferret sends it.

use axum::http::StatusCode;
use entity::users;
use oxy_auth::token::hash_token;
use oxy_auth::token::service::{self, ServiceToken};
use sea_orm::EntityTrait;

use super::stack::{Reach, flat_api, make_staff, mint, published_app};
use super::{Fixture, call, fixture};

/// ferret's token in these tests: the prefix and 48 alphanumerics, the shape
/// the infrastructure generates.
pub(super) const FERRET_TOKEN: &str = "oxysvc_Fe7retTestToken0nly4ServiceTokenTests0123456789";

/// The token's SHA-256 as `OXY_SERVICE_TOKENS` spells it: 64 lowercase hex.
fn sha256_hex(token: &str) -> String {
    hash_token(token)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The `ferret` service at `global_admin`, installed for this test process,
/// with its `users` row created as boot creates it.
pub(super) async fn with_ferret(fx: &Fixture) -> &'static ServiceToken {
    let sha256 = sha256_hex(FERRET_TOKEN);
    let ferret = ServiceToken::new("ferret", "global_admin", &sha256).expect("a valid entry");
    service::install(vec![ferret]).expect("install the service");
    service::ensure_principals(&fx.db)
        .await
        .expect("create the principal");
    service::authenticate(FERRET_TOKEN).expect("the token names ferret")
}

/// `GET uri` on the flat tree with `key` in `x-api-key`.
async fn get_with_key(key: &str, uri: &str) -> StatusCode {
    call(flat_api(), "GET", uri, &[("x-api-key", key)], None)
        .await
        .0
}

/// ferret's four reads, with its own query parameters.
fn ferret_reads(health: &str) -> Vec<String> {
    let until = chrono::Utc::now();
    let since =
        (until - chrono::Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let until = until.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    vec![
        "/customer-apps?limit=200&offset=0".to_string(),
        "/customer-apps/fleet-health?limit=200&offset=0".to_string(),
        health.to_string(),
        format!(
            "/customer-apps/function-failures?{}",
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("since", &since)
                .append_pair("until", &until)
                .append_pair("limit", "500")
                .finish()
        ),
    ]
}

#[tokio::test]
async fn a_service_token_reads_every_route_ferret_reads() {
    let fx = fixture().await;
    with_ferret(&fx).await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let health = format!(
        "/customer-apps/acme-{}/{}/health",
        fx.org_id.simple(),
        app.slug
    );

    for uri in ferret_reads(&health) {
        let (status, body) = call(
            flat_api(),
            "GET",
            &uri,
            &[("x-api-key", FERRET_TOKEN)],
            None,
        )
        .await;
        if uri == health {
            // A verdict, not a refusal: the app has no build.
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {body}");
            assert_eq!(body["oxy_app_health"], "fail");
        } else {
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
    }
    let (_, list) = call(
        flat_api(),
        "GET",
        "/customer-apps?limit=200&offset=0",
        &[("authorization", &format!("Bearer {FERRET_TOKEN}"))],
        None,
    )
    .await;
    assert_eq!(
        list["items"][0]["id"],
        app.id.to_string(),
        "as a bearer too, and across every org"
    );
}

#[tokio::test]
async fn a_token_no_service_matches_is_401() {
    let fx = fixture().await;
    with_ferret(&fx).await;
    let mut wrong = FERRET_TOKEN.to_string();
    wrong.pop();
    wrong.push('Z');
    let other = "oxysvc_0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKL";
    for key in [wrong.as_str(), other, "oxysvc_"] {
        assert_eq!(
            get_with_key(key, "/customer-apps").await,
            StatusCode::UNAUTHORIZED,
            "{key:?}"
        );
    }
    // The hash alone is not a credential either.
    assert_eq!(
        get_with_key(&sha256_hex(FERRET_TOKEN), "/customer-apps").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_global_admin_service_reaches_what_a_global_admin_token_reaches() {
    let fx = fixture().await;
    with_ferret(&fx).await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let (_, admin) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;

    let reads = [
        "/admin/orgs-meta".to_string(),
        "/admin/users".to_string(),
        "/admin/audit".to_string(),
        "/customer-apps".to_string(),
        "/customer-apps/storage".to_string(),
        "/customer-apps/oxy-access".to_string(),
        format!("/customer-apps/{}", app.id),
        format!("/customer-apps/{}/functions", app.id),
    ];
    for uri in &reads {
        let as_admin = get_with_key(&admin, uri).await;
        assert!(
            as_admin.is_success(),
            "{uri}: the Global Admin's control is {as_admin}"
        );
        assert_eq!(get_with_key(FERRET_TOKEN, uri).await, as_admin, "{uri}");
    }

    // The two deliberate differences: no grant management, and no owner tier.
    assert!(get_with_key(&admin, "/admin/app-admins").await.is_success());
    assert_eq!(
        get_with_key(FERRET_TOKEN, "/admin/app-admins").await,
        StatusCode::FORBIDDEN,
        "a service cannot see or edit who holds staff standing"
    );
    let (status, _) = call(
        flat_api(),
        "POST",
        "/admin/app-admins",
        &[("x-api-key", FERRET_TOKEN)],
        Some(serde_json::json!({ "email": "someone@example.com", "role": "app_operator" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a service cannot grant standing"
    );
    assert_eq!(
        get_with_key(FERRET_TOKEN, "/admin/billing/prices").await,
        StatusCode::FORBIDDEN,
        "owner-only stays owner-only"
    );
}

#[tokio::test]
async fn the_principal_is_a_users_row_with_no_email_created_once() {
    let fx = fixture().await;
    let ferret = with_ferret(&fx).await;
    service::ensure_principals(&fx.db)
        .await
        .expect("idempotent");

    let row = users::Entity::find_by_id(ferret.user_id())
        .one(&fx.db)
        .await
        .unwrap()
        .expect("the principal's row");
    assert_eq!(row.name, "service:ferret");
    assert_eq!(row.email, None, "in no staff audience and no grant list");
    assert_eq!(row.status, users::UserStatus::Active);
}
