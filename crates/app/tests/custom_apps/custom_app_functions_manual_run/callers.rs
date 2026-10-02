//! The caller shapes the refusal tests hand a handler directly, with the
//! extractors the router would build.
//!
//! No caller shape passes the mount's own guards (`manage_apps`) and lacks
//! `develop_apps`, and the test stack leaves out `auth_middleware`'s token
//! path, so a caller without reach and a publish token are only reachable by
//! calling the handler. The mount's guards are
//! `custom_app_functions_manual_run_guards`'s.

use axum::Extension;
use axum::extract::Query;
use entity::users::UserStatus;
use oxy_app::server::api::admin::apps::environment_scope::EnvironmentQuery;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use uuid::Uuid;

use crate::custom_app_functions_fixture::Tenant;

pub(super) fn staff(t: &Tenant) -> AuthenticatedUserExtractor {
    user(t.guest_id, LOCAL_GUEST_EMAIL)
}

/// Someone oxy-authz does not let open a non-production environment.
pub(super) fn outsider() -> AuthenticatedUserExtractor {
    user(Uuid::new_v4(), "tenant-admin@customer.example")
}

pub(super) fn user(id: Uuid, email: &str) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id,
        email: Some(email.to_string()),
        name: "Checks".to_string(),
        picture: None,
        status: UserStatus::Active,
    })
}

pub(super) fn publish_token(app_id: Uuid) -> Option<Extension<AppPublishTokenAuth>> {
    Some(Extension(AppPublishTokenAuth {
        token_id: Uuid::new_v4(),
        app_id: Some(app_id),
        machine_identity: None,
    }))
}

pub(super) fn environment(name: &str) -> Query<EnvironmentQuery> {
    Query(EnvironmentQuery {
        environment: Some(name.to_string()),
    })
}
