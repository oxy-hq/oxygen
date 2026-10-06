//! A **sandbox agent token** (`oxy_sbx_`) on the custom-app serving paths:
//! `/fn` and `/logs` (sandbox agent credential design §1 rows F1 and L1, §4).
//!
//! Both name their app by `<org>/<app>` slugs, which the route allow-list
//! cannot check, and both authenticate through `authenticate_and_authorize`,
//! which answers by the token's **workspace** grant — so, alone, it would
//! admit every app published from that workspace. What is here closes that:
//!
//! * [`admits_app`] — the app the slugs resolve to must be one the token is
//!   granted. Any other is answered as an unknown app.
//! * [`resolve_app_role_in`] — the token's app role is decided for the one
//!   environment the request is about: `admin` on a sandbox it created
//!   (decision 7), nothing anywhere else.
//! * [`fresh_user`] — the minter's `users` row is read on every request, past
//!   the 60 s user cache, so a change to the minter is seen at once.
//!
//! Every function is the unchanged path for every other credential, with no
//! extra read: the custom-app serving chain stays fully cached for them.

use std::future::Future;

use axum::response::Response;
use entity::{app_members, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::token::CredentialContext;
use oxy_auth::token::usage::{self, UsageSample};
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::UserService;
use oxy_shared::errors::OxyError;
use oxy_telemetry::http_trace::RequestTokenId;
use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use super::custom_apps_auth::resolve_app_role;
use super::custom_apps_non_production::environment_facet;

/// Whether the request's credential is a sandbox agent token.
pub(crate) fn is_agent(credential: Option<&CredentialContext>) -> bool {
    credential.is_some_and(CredentialContext::is_sandbox_agent)
}

/// Whether `caller` may be answered about `app` at all. A sandbox agent token
/// only about an app one of its grants names; everyone else, always — their
/// own checks follow.
pub(crate) fn admits_app(caller: &oxy_server_authz::Caller, app: &apps::Model) -> bool {
    match caller.sandbox_agent() {
        Some(reach) => reach
            .apps
            .iter()
            .any(|granted| granted.app_id == app.id && granted.org_id == app.org_id),
        None => true,
    }
}

/// The minter behind a sandbox agent token, read now. `Ok(None)` when no such
/// user exists any more.
pub(crate) async fn fresh_user(identity: &Identity) -> Result<Option<AuthenticatedUser>, OxyError> {
    UserService::find_user_by_identity(identity).await
}

/// [`resolve_app_role`] for a call that knows **which environment** it is
/// about.
///
/// For every caller but a sandbox agent token this is `resolve_app_role`,
/// unchanged: an environment changes nothing without such a token. For the
/// token it asks `Ring::AppAdmin` of the app **in that environment**, which
/// holds on a sandbox the token created and nowhere else — so its `/fn` calls
/// there see `appRole: admin`, and production's logs stay closed.
pub(crate) async fn resolve_app_role_in(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<&'static str>, DbErr> {
    if !caller.is_sandbox_agent() {
        return resolve_app_role(db, caller, app).await;
    }
    // Boxed: this is awaited inside a function run's future, which is large
    // and stack-sensitive; the token's branch adds a pointer to it.
    Box::pin(agent_app_role(db, caller, app, environment)).await
}

/// The token's app role in `environment`: `admin` on a sandbox it created.
async fn agent_app_role(
    db: &DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<&'static str>, DbErr> {
    let facet = environment_facet(db, app, environment).await?;
    let resource =
        oxy_authz::Resource::app_with_visibility(app.id, app.org_id, app.is_restricted())
            .published_from(app.project_id)
            .in_environment(facet);
    let admin = match oxy_server_authz::loader::load_principal_facts_scoped(db, caller, false).await
    {
        Some(facts) => oxy_authz::allows(&facts, oxy_authz::Action::AppAdmin, &resource),
        // Facts unknown (a DB blip) → not admin. Fail closed.
        None => false,
    };
    Ok(admin.then_some(app_members::ROLE_ADMIN))
}

tokio::task_local! {
    /// The API token the request in flight authenticated with, for
    /// [`UsageProbe::counted`]. Set by [`seen`], inside that scope only.
    static SEEN: std::cell::Cell<Option<Uuid>>;
}

/// Note the token a request authenticated with, for the count taken when the
/// request ends. A no-op for a session and a legacy key, and outside
/// [`UsageProbe::counted`].
pub(crate) fn seen(credential: Option<&CredentialContext>) {
    if let Some(credential) = credential.filter(|c| !c.is_legacy()) {
        let _ = SEEN.try_with(|slot| slot.set(Some(credential.token_id)));
    }
}

/// Counts one request against the **new-format API token** it used — personal,
/// service-account, `ci` or sandbox agent — on the two paths that authenticate
/// inline and so sit outside `/api`'s usage layer: `/fn` and `/logs`. What is
/// recorded is what that layer records — the token id, the response status,
/// the client address and user agent, and the route template — and the token
/// id is put on the response for the request span.
///
/// Free for everyone else, and cheap for the token: whether to count is read
/// off the presented prefix (`presents_api_token`, one header look and no
/// database read), the token's id is the one authentication resolved anyway,
/// and the sample goes to the in-memory accumulator the `/api` layer feeds. A
/// session, an anonymous request and a legacy key present no such prefix and
/// run their handler exactly as before — the serving chain stays fully cached
/// for them, and a legacy key is counted here no more than it ever was.
pub(crate) struct UsageProbe(Option<(Option<String>, Option<String>)>);

impl UsageProbe {
    pub(crate) fn of(headers: &axum::http::HeaderMap) -> Self {
        Self(oxy_auth::token::presents_api_token(headers).then(|| {
            (
                oxy_app_core::forwarded::client_ip(headers),
                oxy_app_core::audit::user_agent(headers),
            )
        }))
    }

    /// Run `handler`; count its response if the token authenticated.
    pub(crate) async fn counted<F>(self, route: &'static str, handler: F) -> Response
    where
        F: Future<Output = Response>,
    {
        let Some((ip, user_agent)) = self.0 else {
            return handler.await;
        };
        SEEN.scope(std::cell::Cell::new(None), async move {
            let mut response = handler.await;
            let Some(token_id) = SEEN.with(std::cell::Cell::get) else {
                return response;
            };
            response
                .extensions_mut()
                .insert(RequestTokenId(token_id.to_string()));
            usage::record(UsageSample {
                token_id,
                status: response.status().as_u16(),
                ip,
                user_agent,
                route: Some(route.to_string()),
                at: chrono::Utc::now(),
            });
            response
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::custom_apps_agent_fixture as fixture;
    use oxy_server_authz::Caller;

    const ORG: Uuid = Uuid::from_u128(2);
    const APP: Uuid = Uuid::from_u128(1);
    const WORKSPACE: Uuid = Uuid::from_u128(3);

    fn app(id: Uuid, org_id: Uuid) -> apps::Model {
        fixture::app(id, org_id, WORKSPACE)
    }

    fn agent() -> Caller {
        let credential = fixture::credential(Uuid::from_u128(0x70), ORG, APP, WORKSPACE);
        Caller::from_user(&fixture::user(Some(credential)))
    }

    /// `/fn` and `/logs` name their app by slugs. The token's workspace grant
    /// covers every app of the workspace; only the one its grant names is
    /// answered.
    #[test]
    fn a_token_is_answered_only_about_an_app_it_is_granted() {
        let agent = agent();
        assert!(admits_app(&agent, &app(APP, ORG)));
        let sibling = app(Uuid::from_u128(9), ORG);
        assert!(!admits_app(&agent, &sibling), "same workspace, another app");
        let elsewhere = app(APP, Uuid::from_u128(8));
        assert!(!admits_app(&agent, &elsewhere), "the id under another org");
    }

    /// No other caller is narrowed here.
    #[test]
    fn every_other_caller_is_left_to_its_own_checks() {
        let person = Caller::from_user(&fixture::user(None));
        assert!(admits_app(&person, &app(Uuid::from_u128(9), ORG)));
        assert!(!is_agent(None));
        let credential = fixture::credential(Uuid::from_u128(0x70), ORG, APP, WORKSPACE);
        assert!(is_agent(Some(&credential)));
    }
}
