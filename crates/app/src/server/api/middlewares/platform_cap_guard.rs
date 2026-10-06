//! Per-section capability gate for the staff console.
//!
//! `oxy_owner_or_app_admin_guard` is the **door**: it answers "are you staff at all"
//! and wraps the whole `/admin/*` nest. It used to be the only question asked, which is
//! why every staff member reached every section — the console had one lock and everyone
//! holding a key held the same key.
//!
//! This is the lock on each room. A sub-router carries
//! `route_layer(require(Action::Platform*))` and the model decides from the caller's
//! capability grant, exactly the way `billing` and `app_admins` already escalate to
//! [`oxy_owner_guard`](super::oxy_owner_guard) — a pattern this generalises rather than
//! replaces.
//!
//! **Scope is deliberately not consulted here.** `Resource::platform()` has no org to
//! check a scope against, so a scoped operator passes this gate and the *handler*
//! narrows the rows it returns (`PrincipalFacts::platform_scope`). Capabilities gate
//! verbs; scope filters rows. A guard that tried to enforce scope would 403 a scoped
//! operator out of their own console.

use std::future::Future;
use std::pin::Pin;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use oxy_auth::types::AuthenticatedUser;

use crate::server::api::middlewares::oxy_app_admin_guard::is_oxy_app_admin;
use crate::server::authz::{Action, Caller, Resource, enforce, globals, loader};

type GuardFuture = Pin<Box<dyn Future<Output = Result<Response, StatusCode>> + Send>>;

/// A `route_layer`-able guard requiring `action`'s capability.
///
/// ```ignore
/// .merge(apps::router().route_layer(middleware::from_fn(require(Action::PlatformApps))))
/// ```
pub fn require(
    action: Action,
) -> impl Clone + Send + Sync + 'static + Fn(Request<Body>, Next) -> GuardFuture {
    move |request, next| Box::pin(enforce_cap(action, request, next))
}

async fn enforce_cap(
    action: Action,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let user = request
        .extensions()
        .get::<AuthenticatedUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // The oracle: what the shipped console gate grants today — any staff standing.
    // Passing it (rather than `true`) keeps `enforce`'s conjunction meaningful, so this
    // layer can only ever SUBTRACT from the reach the console already had. A
    // mis-modeled capability therefore produces a wrong 403 — loud, attributable, one
    // line to revert — never a hole.
    // Named separately from `legacy`, because the two halves behave differently when the
    // database is unreachable: the owner half is an env read that survives any outage,
    // the admin half is a table lookup that does not — and after this PR the admin half
    // is true for App Operators too, so it is no longer a stand-in for "may use this
    // section". Fusing them is what let the unknown-standing arm below look reasonable.
    //
    // Both halves are read for the CALLER — the user and the credential the request
    // arrived with — so an API token that does not carry platform standing is not
    // staff here, whoever its bearer is.
    let caller = Caller::of_request(request.extensions(), &user);
    let is_owner = globals::is_global_owner(&caller);
    let legacy = is_owner || is_oxy_app_admin(&caller).await;

    let facts = match oxy::database::client::establish_connection().await {
        Ok(db) => loader::load_platform_facts(&db, &caller).await,
        Err(e) => {
            tracing::error!(
                target: "authz",
                error = %e,
                "no database connection in the capability guard"
            );
            None
        }
    };

    let allowed = match facts {
        Some(facts) => enforce(
            "guard.platform_cap",
            &facts,
            action,
            &Resource::platform(),
            legacy,
        ),
        // **Unknown standing refuses.** This used to fall back to `legacy`, borrowing the
        // door guard's fail-safe — but the two guards ask different questions and the
        // borrowed reasoning stopped holding when this PR widened `app_admins`.
        //
        // For the door, "are you staff at all" IS the legacy question, so oracle and
        // model coincide and deferring costs nothing. Here the question is one
        // capability per section, while `is_oxy_app_admin` now returns true for App
        // Operator rows too. So `legacy` is strictly BROADER than the modelled verdict
        // for every role but Global Admin, and deferring to it doesn't defer — it
        // promotes: an App Operator with unknown standing would pass `PlatformOrgs`,
        // `PlatformOperate`, all of them. Internal jobs, compiles and the explorer have
        // no second gate behind this layer to catch it.
        //
        // Refusing matches what the other two readers of `load_platform_facts`' `None`
        // already do (`orgs_admin::create_org`, `app_scope_guard`). One contract, three
        // call sites, one answer.
        None => {
            // Except for the Global Owner, whose standing is an env allow-list that
            // never needed the database — the rule `platform_standing_offline` states.
            // No outage may take away a flag no database ever granted.
            if is_owner {
                return Ok(next.run(request).await);
            }
            tracing::error!(
                target: "authz",
                ?action,
                "platform standing unreadable in the capability guard — refusing"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}
