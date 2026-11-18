//! The route handlers `public_routes` mounts: each admits the caller
//! ([`super::door`]), reads its request ([`super::requests`]) and calls the
//! operation, nothing more.
//!
//! `GET /customer-apps/{org}/events` shares its path with the bundle SDK's
//! `POST /customer-apps/{project_id}/events` (`custom_apps_activity`), so it is
//! mounted on that path and reads the segment as the org's slug.

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query};
use axum::http::HeaderMap;
use axum::response::Response;

use super::reply::{ApiError, Counted, respond};
use super::requests::{
    self, BulkRedeliverBody, DiscardBody, EnvironmentQuery, FeedQuery, FindQuery, PurgeBody,
    RedeliverBody, ReplayBody, RootQuery, SwitchBody,
};
use super::tables::ResyncBody;
use super::{dead, destructive, door, feed, redeliver, replay, show, switch, tables, types};

type Q<T> = Result<Query<T>, QueryRejection>;
type B<T> = Result<Json<T>, JsonRejection>;

fn query<T>(q: Q<T>) -> Result<T, ApiError> {
    q.map(|Query(q)| q)
        .map_err(|e| ApiError::BadRequest("invalid_query", e.body_text()))
}

fn body<T>(b: B<T>) -> Result<T, ApiError> {
    b.map(|Json(b)| b)
        .map_err(|e| ApiError::BadRequest("invalid_body", e.body_text()))
}

/// Nothing to redeliver: a listener the org does not have.
fn none(dry_run: bool) -> Counted {
    Counted {
        count: 0,
        remaining: 0,
        dry_run,
    }
}

/// `GET /customer-apps/{org}/events/types`
pub async fn list_types(Path(org): Path<String>, headers: HeaderMap) -> Response {
    respond(
        async {
            let a = door::org_reader(&headers, &org).await?;
            types::list(&a.db, a.org_id).await
        }
        .await,
    )
}

/// `GET /customer-apps/{org}/events/{event_id}`
pub async fn show_event(Path((org, id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    respond(
        async {
            let a = door::org_reader(&headers, &org).await?;
            let id = requests::uuid(&id, "event_not_found")?;
            show::view(&a.db, show::event(&a.db, a.org_id, id).await?).await
        }
        .await,
    )
}

/// `GET /customer-apps/{org}/events/find?type=&key=&environment=`
pub async fn find_event(Path(org): Path<String>, q: Q<FindQuery>, headers: HeaderMap) -> Response {
    respond(
        async {
            let a = door::org_reader(&headers, &org).await?;
            let id = requests::found_id(a.org_id, &query(q)?)?;
            show::view(&a.db, show::event(&a.db, a.org_id, id).await?).await
        }
        .await,
    )
}

/// `GET /customer-apps/{org}/events?root=`
pub async fn list_chain(Path(org): Path<String>, q: Q<RootQuery>, headers: HeaderMap) -> Response {
    respond(
        async {
            let a = door::org_reader(&headers, &org).await?;
            show::chain(&a.db, a.org_id, requests::root(&query(q)?)?).await
        }
        .await,
    )
}

/// `GET /customer-apps/{org}/{app}/events/feed?since=&environment=`
pub async fn app_feed(
    Path((org, app)): Path<(String, String)>,
    q: Q<FeedQuery>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let q = query(q).unwrap_or_default();
            let a = door::app_reader(&headers, &org, &app, q.environment.as_deref()).await?;
            let since = feed::since(q.since.as_deref())?;
            let env = a.environment.name();
            feed::feed(&a.db, (a.app.id, &a.app.slug), &env, since).await
        }
        .await,
    )
}

/// `GET /customer-apps/{org}/{app}/events/dead?environment=`
pub async fn app_dead(
    Path((org, app)): Path<(String, String)>,
    q: Q<EnvironmentQuery>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let q = query(q).unwrap_or_default();
            let a = door::app_reader(&headers, &org, &app, q.environment.as_deref()).await?;
            dead::dead(&a.db, (a.app.id, &a.app.slug), &a.environment.name()).await
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/events/{event_id}/redeliver`
pub async fn redeliver_event(
    Path((org, id)): Path<(String, String)>,
    headers: HeaderMap,
    b: B<RedeliverBody>,
) -> Response {
    respond(
        async {
            let a = door::org_manager(&headers, &org).await?;
            let event =
                show::event(&a.db, a.org_id, requests::uuid(&id, "event_not_found")?).await?;
            let b = body(b)?;
            match requests::single(&a.db, a.org_id, event.id, &b).await? {
                Some(selection) => redeliver::redeliver(&a.db, &selection, b.dry_run).await,
                None => Ok(none(b.dry_run)),
            }
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/events/redeliver`
pub async fn redeliver_bulk(
    Path(org): Path<String>,
    headers: HeaderMap,
    b: B<BulkRedeliverBody>,
) -> Response {
    respond(
        async {
            let a = door::org_manager(&headers, &org).await?;
            let b = body(b)?;
            match requests::bulk(&a.db, a.org_id, &b).await? {
                Some(selection) => redeliver::redeliver(&a.db, &selection, b.dry_run).await,
                None => Ok(none(b.dry_run)),
            }
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/events/replay`
pub async fn replay_events(
    Path(org): Path<String>,
    headers: HeaderMap,
    b: B<ReplayBody>,
) -> Response {
    respond(
        async {
            let a = door::org_manager(&headers, &org).await?;
            let r = requests::replay(&a.db, a.org_id, &body(b)?).await?;
            replay::replay(&a.db, &r).await
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/events/discard`
pub async fn discard_deliveries(
    Path(org): Path<String>,
    headers: HeaderMap,
    b: B<DiscardBody>,
) -> Response {
    respond(
        async {
            let a = door::org_manager(&headers, &org).await?;
            let d = requests::discard(&a.db, a.org_id, &body(b)?).await?;
            destructive::discard(&a.db, &a.actor, &d).await
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/events/purge`
pub async fn purge_events(
    Path(org): Path<String>,
    headers: HeaderMap,
    b: B<PurgeBody>,
) -> Response {
    respond(
        async {
            let a = door::org_manager(&headers, &org).await?;
            let p = requests::purge(a.org_id, &body(b)?)?;
            destructive::purge(&a.db, &a.actor, &p).await
        }
        .await,
    )
}

/// `POST /customer-apps/{org}/{app}/events/resync`
pub async fn resync_tables(
    Path((org, app)): Path<(String, String)>,
    headers: HeaderMap,
    b: B<ResyncBody>,
) -> Response {
    respond(
        async {
            let a = door::app_manager(&headers, &org, &app).await?;
            let actor = a.actor.as_ref().ok_or(ApiError::Forbidden("forbidden"))?;
            let req = body(b)?;
            let mut refused = Vec::new();
            for other in tables::sharers_of(&a.db, &a.app, &req).await? {
                if !door::may_manage(&headers, &a.db, &other).await {
                    refused.push(other.slug);
                }
            }
            tables::resync(&a.db, actor, &a.app, &req, refused).await
        }
        .await,
    )
}

/// `PUT /customer-apps/{org}/{app}/environments/{env}/events`
pub async fn set_events_switch(
    Path((org, app, env)): Path<(String, String, String)>,
    headers: HeaderMap,
    b: B<SwitchBody>,
) -> Response {
    respond(
        async {
            let a = door::app_manager(&headers, &org, &app).await?;
            let enabled = requests::enabled(&body(b)?)?;
            let actor = a.actor.as_ref().ok_or(ApiError::Forbidden("forbidden"))?;
            switch::set(&a.db, actor, &a.app, &env, enabled).await
        }
        .await,
    )
}
