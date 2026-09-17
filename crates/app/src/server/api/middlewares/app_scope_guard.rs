//! Scope enforcement for the custom-app admin surfaces, in one place.
//!
//! `platform_cap_guard` answers "may you use this section" (a capability). It
//! deliberately does not consult **scope**, because `Resource::platform()` has no org to
//! check against. Scope is therefore the handler's job — and the custom-app console has
//! roughly twenty `/{id}`-shaped routes: publish, unpublish, rollback, builds, functions,
//! function runs, access, teams, members, activity, secrets, publishers.
//!
//! Asking each of those to remember a scope check is how the ~170-site authorization
//! scatter got built in the first place. One of them forgetting is not a cosmetic bug: a
//! grant bounded to org A could write a secret into org B's app, or rewrite who may open
//! it. So the check lives here, layered once over the whole tree, and reads the app id
//! straight out of the matched path.
//!
//! **What this does NOT cover**, because the id isn't in the path. Each is numbered, and
//! the handler carrying the check cites its number back:
//! * **#1 `create_app`** — the org arrives in the body; checked in the handler.
//! * **#2 the `batch/*` endpoints** — ids arrive in the body; checked per id via
//!   `split_by_scope` in `handlers.rs`.
//! * **#3 list-shaped reads that name no app** — `list_apps`, and `list_grants`'s
//!   workspace picker; filtered as rows through `scope_org_filter`.
//! * **#4 the `storage/*` routes** — the target is a `?appId=` query param
//!   (`storage/history`), an `{org_id}` path segment (`storage/meter/{org_id}`), or absent
//!   entirely (`storage`, `storage/sweep`). Each checks itself: list-shaped reads filter
//!   rows through `scope_org_filter`, targeted ones go through `handlers::org_in_scope`
//!   and 404.
//!
//! Those four are the complete exception list. Keep it that way: a new route whose
//! target org isn't in `{id}` needs its own check, and should say so out loud — #3 is the
//! worked example of doing that for a target the path can't express.

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use entity::prelude::Apps;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::server::authz::globals;

/// Enforce the caller's grant scope on whatever app `{id}` names.
///
/// Passes through untouched when there is nothing to check — no `{id}` in the matched
/// path (`/apps`, `/apps/health`, `/templates`), an unparseable id, or an app that
/// doesn't exist. Those all belong to the handler, which returns its own 404/400; this
/// layer only ever *subtracts*.
pub async fn enforce_app_scope(request: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    let Some(user) = request.extensions().get::<AuthenticatedUser>().cloned() else {
        // Unauthenticated requests never reach a scoped decision — the outer auth layer
        // owns that verdict, and duplicating it here would be a second opinion.
        return Ok(next.run(request).await);
    };

    let (mut parts, body) = request.into_parts();
    let app_id = Path::<HashMap<String, String>>::from_request_parts(&mut parts, &())
        .await
        .ok()
        .and_then(|Path(params)| params.get("id").and_then(|id| Uuid::parse_str(id).ok()));
    let request = Request::from_parts(parts, body);

    let Some(app_id) = app_id else {
        return Ok(next.run(request).await);
    };

    let db = match oxy::database::client::establish_connection().await {
        Ok(db) => db,
        // Refuse, exactly as the query arm below does. Pool-acquisition failure and
        // query failure are the SAME outage — which one a request hits is timing — so
        // answering them differently means a DB blip either refuses or admits an
        // unscoped request depending on where the pool happened to give out.
        //
        // This arm used to defer, reasoning that "every handler behind this needs the
        // same connection and will fail honestly". That is precisely the argument the
        // query arm declined: it makes the fence's safety contingent on a property of
        // every current and future handler behind it, rather than on this layer. A
        // handler that can answer without the database — or one added later that can —
        // would be served unfenced.
        Err(e) => {
            tracing::error!(
                target: "authz",
                error = %e,
                %app_id,
                "no database connection in the scope guard — refusing rather than passing through"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let app = match Apps::find_by_id(app_id).one(&db).await {
        Ok(Some(app)) => app,
        // No such app — the handler's 404 to give, not ours.
        Ok(None) => return Ok(next.run(request).await),
        // An errored lookup is NOT "no such app". Passing it through would send an
        // unscoped request to a handler that trusts this layer to have fenced it, so a
        // blip on this query becomes a scope bypass. Refuse — and refuse on this
        // layer's own terms, not because "the handler would fail anyway": that reasoning
        // outsources the fence's safety to every handler behind it.
        Err(e) => {
            tracing::error!(
                target: "authz",
                error = %e,
                %app_id,
                "app lookup failed in the scope guard — refusing rather than passing through"
            );
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if !globals::platform_reaches(
        &db,
        user.email.as_deref().unwrap_or(""),
        oxy_authz::Cap::ManageApps,
        app.org_id,
    )
    .await
    {
        // 404, not 403: an operator with no reach into an org must not be able to
        // confirm that one of its apps exists by probing ids.
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(next.run(request).await)
}
