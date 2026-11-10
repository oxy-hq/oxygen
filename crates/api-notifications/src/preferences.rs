//! What a person has turned off, and the routes to change it — the shape
//! from before the settings document (`settings`), kept working.
//!
//! One switch per (app, kind, channel), listed for the kinds the person has
//! actually been sent or has set — the inbox is the catalogue, so a kind
//! appears the first time an app uses it. Push, email and Slack can be muted;
//! the inbox cannot, because it is the record that the person was told. A kind
//! the app declared critical ignores mutes, and says so here.
//!
//! A mute is a channel set off at a node of `notification_node_settings`
//! and an unmute sets it back to inherit (`settings::store`); `muted` lists
//! the channels set off at the kind itself.
//!
//! Self-scoped like the inbox: every query filters `user_id = me`.

use std::collections::{BTreeMap, HashSet};

use axum::Json;
use axum::http::StatusCode;
use entity::{apps, device_tokens, notification_sends, notifications, users};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, PaginatorTrait, QueryFilter, QuerySelect,
};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use super::delivery::Channel;
use super::inbox_view::InboxApp;
use super::input::{is_reserved_kind, valid_kind};
use super::settings::model::Node;
use super::settings::store;

#[derive(Debug, Clone, Serialize)]
pub struct KindPreference {
    pub kind: String,
    /// Channels turned off for this kind.
    pub muted: Vec<&'static str>,
    /// Declared critical by the app: mutes are kept but do not apply.
    pub critical: bool,
    /// The channels this kind can reach someone on: the only switches that do
    /// anything. Every channel for an app, since each call picks email and
    /// Slack itself; a platform kind's fixed set (`platform::kinds::channels`).
    pub channels: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct AppPreferences {
    pub app: InboxApp,
    pub kinds: Vec<KindPreference>,
}

/// Whether each channel can reach this person at all, so the settings page
/// can say "add a device" rather than show a switch that does nothing.
#[derive(Debug, Clone, Serialize)]
pub struct Reachability {
    pub push_devices: u64,
    pub email: bool,
}

#[derive(Debug, Serialize)]
pub struct PreferencesResponse {
    pub apps: Vec<AppPreferences>,
    /// Oxygen's own notices (`oxy.*` kinds) this person has received or muted.
    pub platform: Vec<KindPreference>,
    pub reach: Reachability,
}

#[derive(Debug, Deserialize)]
pub struct SetPreference {
    /// The app whose kind or category this is; `null` for Oxygen's notices.
    pub app_id: Option<Uuid>,
    /// One kind. Exactly one of `kind` and `scope`.
    pub kind: Option<String>,
    /// Everything above a kind: `""` for the whole source, or a category
    /// path, which covers its subtree (`scope_mutes`).
    pub scope: Option<String>,
    /// `push` | `email` | `slack`.
    pub channel: String,
    pub muted: bool,
}

/// `GET /api/notifications/preferences`
#[instrument(skip_all)]
pub async fn get_preferences(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<PreferencesResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    preferences_for(&db, &user)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// The preferences `user` sees. Separate from the route so it can be driven
/// against a test database.
pub async fn preferences_for(
    db: &DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
) -> Result<PreferencesResponse, DbErr> {
    let blocked: HashSet<Uuid> = user.blocked_orgs().iter().copied().collect();
    Ok(PreferencesResponse {
        apps: app_preferences(db, user.id, &blocked).await?,
        platform: super::platform::preferences::kinds(db, user.id).await?,
        reach: reachability(db, user.id).await?,
    })
}

/// `PUT /api/notifications/preferences` — mute or unmute one channel for one
/// kind from one app, or (`app_id: null`) for one kind of platform notice; or,
/// with `scope` instead of `kind`, for a whole source or a category subtree.
#[instrument(skip_all, fields(channel = %body.channel))]
pub async fn set_preference(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<SetPreference>,
) -> Result<StatusCode, StatusCode> {
    let channel = Channel::parse(&body.channel).ok_or(StatusCode::BAD_REQUEST)?;
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let kind = match (body.kind.as_deref(), body.scope.as_deref()) {
        (Some(kind), None) => kind,
        (None, Some(scope)) => {
            return super::scope_mutes::set(&db, &user, body.app_id, scope, channel, body.muted)
                .await;
        }
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    set_kind(&db, &user, body.app_id, kind, channel, body.muted).await
}

/// The kind form of [`set_preference`].
async fn set_kind(
    db: &DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
    app_id: Option<Uuid>,
    kind: &str,
    channel: Channel,
    muted: bool,
) -> Result<StatusCode, StatusCode> {
    if !valid_kind(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let Some(app_id) = app_id else {
        return super::platform::preferences::set(db, user.id, kind, channel, muted).await;
    };
    // An app's kind never wears the platform's prefix (publish and
    // `ctx.notify` both refuse one), so a mute naming one is a mistake.
    if is_reserved_kind(kind) {
        return Err(StatusCode::BAD_REQUEST);
    }
    // 404 for an app this person cannot be told by, as for one that does not
    // exist: a mute is not a way to probe the app registry.
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter(|a| user.reaches_org(a.org_id))
        .ok_or(StatusCode::NOT_FOUND)?;
    if !may_be_told_by(db, user.id, &app)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let node = Node::Kind(kind.to_string());
    store::set_channel(
        db,
        user.id,
        Some(app.id),
        &node,
        channel,
        muted.then_some(false),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Whether an app can notify a person: they already hold a notification from
/// it, or they are in its audience. The one decision [`may_be_told_by`] takes
/// for one app and `catalog::audience::told_by_many` for many.
pub fn may_be_told(received: bool, in_audience: bool) -> bool {
    received || in_audience
}

/// Whether `app` can notify this person ([`may_be_told`]), with the audience
/// the rule `ctx.notify` resolves with (`app_audience`). For one app; a list
/// of them is `catalog::audience::told_by_many`, which reads no org's roster.
/// `reaches_org` alone is true for every session, so it bounds nothing here.
pub async fn may_be_told_by(
    db: &DatabaseConnection,
    user_id: Uuid,
    app: &apps::Model,
) -> Result<bool, DbErr> {
    let received = notifications::Entity::find()
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::AppId.eq(app.id))
        .one(db)
        .await?
        .is_some();
    // Asked only when it can change the answer.
    let in_audience = !received
        && oxy_app_core::audience::app_audience(db, app.org_id, app.id)
            .await
            .map_err(DbErr::Custom)?
            .ids
            .contains(&user_id);
    Ok(may_be_told(received, in_audience))
}

/// Every (app, kind) this person was sent or set, with its mutes.
async fn app_preferences(
    db: &DatabaseConnection,
    user_id: Uuid,
    blocked: &HashSet<Uuid>,
) -> Result<Vec<AppPreferences>, DbErr> {
    let received: Vec<(Uuid, String)> = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::AppId)
        .column(notifications::Column::Kind)
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::AppId.is_not_null())
        .distinct()
        .into_tuple::<(Option<Uuid>, String)>()
        .all(db)
        .await?
        .into_iter()
        .filter_map(|(app, kind)| app.map(|a| (a, kind)))
        .collect();
    let mut by_app: BTreeMap<Uuid, BTreeMap<String, Vec<&'static str>>> = BTreeMap::new();
    for (app, kind) in received {
        by_app.entry(app).or_default().entry(kind).or_default();
    }
    for (source, nodes) in store::nodes_for_user(db, user_id).await? {
        let Some(app) = source else { continue };
        for (node, settings) in nodes {
            if let Node::Kind(kind) = node {
                by_app.entry(app).or_default().insert(kind, settings.off());
            }
        }
    }
    let critical = critical_kinds(db, by_app.keys().copied().collect()).await?;
    let app_rows = apps::Entity::find()
        .filter(apps::Column::Id.is_in(by_app.keys().copied()))
        .all(db)
        .await?;
    let mut out = Vec::new();
    for a in app_rows
        .into_iter()
        .filter(|a| !blocked.contains(&a.org_id))
    {
        let kinds = by_app.remove(&a.id).unwrap_or_default();
        out.push(AppPreferences {
            kinds: kinds
                .into_iter()
                .map(|(kind, muted)| KindPreference {
                    critical: critical.contains(&(a.id, kind.clone())),
                    kind,
                    muted,
                    channels: Channel::ALL.iter().map(|c| c.as_str()).collect(),
                })
                .collect(),
            app: InboxApp {
                id: a.id,
                slug: a.slug,
                name: a.name,
            },
        });
    }
    out.sort_by(|x, y| x.app.name.cmp(&y.app.name));
    Ok(out)
}

/// The (app, kind) pairs an app has ever sent as critical.
async fn critical_kinds(
    db: &DatabaseConnection,
    app_ids: Vec<Uuid>,
) -> Result<HashSet<(Uuid, String)>, DbErr> {
    if app_ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(notification_sends::Entity::find()
        .select_only()
        .column(notification_sends::Column::AppId)
        .column(notification_sends::Column::Kind)
        .filter(notification_sends::Column::AppId.is_in(app_ids))
        .filter(notification_sends::Column::Critical.eq(true))
        .distinct()
        .into_tuple::<(Option<Uuid>, String)>()
        .all(db)
        .await?
        .into_iter()
        .filter_map(|(app, kind)| app.map(|a| (a, kind)))
        .collect())
}

pub async fn reachability(db: &DatabaseConnection, user_id: Uuid) -> Result<Reachability, DbErr> {
    let push_devices = device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.eq(user_id))
        .filter(device_tokens::Column::Platform.eq("web"))
        .count(db)
        .await?;
    let email = users::Entity::find_by_id(user_id)
        .one(db)
        .await?
        .and_then(|u| u.email)
        .is_some_and(|e| !e.trim().is_empty());
    Ok(Reachability {
        push_devices,
        email,
    })
}
