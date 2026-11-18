//! One event (`GET …/events/{id}`, and `find`, which derives the id from the
//! key) and one root's chain (`GET …/events?root=`). Only the single event
//! reads its body, from the org's store; the chain never does.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{
    app_event_deliveries as deliveries, app_event_delivery_attempts as attempts, app_event_schemas,
    app_events, app_function_invocations as invocations, apps,
};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::reply::ApiError;
use crate::app_events::deliveries::BODY_GONE;
use crate::app_events::envelope::load_event_data;

/// How far up a causation chain the reply walks: past a publish's depth limit.
const MAX_PARENTS: usize = 16;
/// The most children, and the most events of one root, a reply lists.
const MAX_CHILDREN: u64 = 100;
const MAX_CHAIN: u64 = 1_000;

/// An event's metadata, as every reply that lists one shows it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EventSummary {
    id: Uuid,
    #[serde(rename = "type")]
    event_type: String,
    key: String,
    environment: String,
    published_at: DateTime<Utc>,
    relayed_at: DateTime<Utc>,
    relay_error: Option<String>,
    root_id: Uuid,
    causation_id: Option<Uuid>,
    depth: i32,
    actor: Option<String>,
    invocation_id: Option<Uuid>,
    data_bytes: i32,
}

impl From<&app_events::Model> for EventSummary {
    fn from(e: &app_events::Model) -> Self {
        Self {
            id: e.id,
            event_type: e.r#type.clone(),
            key: e.key.clone(),
            environment: e.environment.clone(),
            published_at: e.published_at.with_timezone(&Utc),
            relayed_at: e.relayed_at.with_timezone(&Utc),
            relay_error: e.relay_error.clone(),
            root_id: e.root_id,
            causation_id: e.causation_id,
            depth: e.depth,
            actor: e.actor.clone(),
            invocation_id: e.invocation_id,
            data_bytes: e.data_bytes,
        }
    }
}

/// The single-event reply.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EventView {
    #[serde(flatten)]
    summary: EventSummary,
    schema_revision: Option<i32>,
    /// The body, read from the org's store; `null` with `dataUnavailable`
    /// saying why when it cannot be.
    data: Option<Value>,
    data_unavailable: Option<String>,
    /// The causation chain upward, nearest first.
    parents: Vec<Related>,
    /// The events this one caused.
    children: Vec<Related>,
    deliveries: Vec<DeliveryView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Related {
    id: Uuid,
    #[serde(rename = "type")]
    event_type: String,
    key: String,
    published_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeliveryView {
    id: Uuid,
    /// `<app>/<function>`.
    listener: String,
    environment: String,
    status: String,
    attempt_count: i32,
    last_error: Option<String>,
    run_id: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Every attempt, oldest first; left out of a chain's listing.
    #[serde(skip_serializing_if = "Option::is_none")]
    attempts: Option<Vec<AttemptView>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AttemptView {
    pub attempt: i32,
    /// `running` while open, else its outcome.
    status: String,
    pub outcome: Option<String>,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    invocation_id: Option<Uuid>,
    /// The invocation the attempt became: metadata only.
    invocation: Option<InvocationView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InvocationView {
    id: Uuid,
    status: String,
    duration_ms: Option<i64>,
    result_status: Option<i16>,
    build_id: Uuid,
    created_at: DateTime<Utc>,
}

/// The reply for `GET …/events?root=`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChainReply {
    root_id: Uuid,
    events: Vec<ChainEvent>,
}

#[derive(Debug, Serialize)]
pub(super) struct ChainEvent {
    #[serde(flatten)]
    summary: EventSummary,
    deliveries: Vec<DeliveryView>,
}

/// `org_id`'s event `id`.
pub(super) async fn event(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<app_events::Model, ApiError> {
    app_events::Entity::find_by_id(id)
        .filter(app_events::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(|e| ApiError::db("read the event", e))?
        .ok_or_else(|| ApiError::NotFound("event_not_found", format!("no event {id} in this org")))
}

/// The single-event reply for `event`, its body read from the org's store.
pub(super) async fn view(
    db: &DatabaseConnection,
    event: app_events::Model,
) -> Result<EventView, ApiError> {
    let read = |e| ApiError::db("read the event's records", e);
    let schema_revision = match event.schema_revision_id {
        Some(id) => app_event_schemas::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(read)?
            .map(|s| s.revision),
        None => None,
    };
    let parents = parents(db, &event).await.map_err(read)?;
    let children = children(db, &event).await.map_err(read)?;
    let deliveries = delivery_views(db, &[event.id], true).await.map_err(read)?;
    let (data, data_unavailable) = body(db, &event).await;
    Ok(EventView {
        summary: EventSummary::from(&event),
        schema_revision,
        data,
        data_unavailable,
        parents,
        children,
        deliveries: deliveries.into_values().flatten().collect(),
    })
}

/// Every event of `root` in `org_id`, in publish order, with its deliveries'
/// statuses.
pub(super) async fn chain(
    db: &DatabaseConnection,
    org_id: Uuid,
    root: Uuid,
) -> Result<ChainReply, ApiError> {
    let read = |e| ApiError::db("read the chain", e);
    let events = app_events::Entity::find()
        .filter(app_events::Column::OrgId.eq(org_id))
        .filter(app_events::Column::RootId.eq(root))
        .order_by_asc(app_events::Column::PublishedAt)
        .order_by_asc(app_events::Column::Depth)
        .order_by_asc(app_events::Column::Id)
        .limit(MAX_CHAIN)
        .all(db)
        .await
        .map_err(read)?;
    let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    let mut deliveries = delivery_views(db, &ids, false).await.map_err(read)?;
    let events = events
        .iter()
        .map(|e| ChainEvent {
            summary: EventSummary::from(e),
            deliveries: deliveries.remove(&e.id).unwrap_or_default(),
        })
        .collect();
    Ok(ChainReply {
        root_id: root,
        events,
    })
}

/// The body, or why it cannot be shown. Never an error: a store that cannot
/// be reached, a body retention deleted and an environment whose store is not
/// relayed all leave the rest of the reply standing.
async fn body(
    db: &DatabaseConnection,
    event: &app_events::Model,
) -> (Option<Value>, Option<String>) {
    match load_event_data(db, event).await {
        Ok(Some(data)) => (Some(data), None),
        Ok(None) => (None, Some(BODY_GONE.to_string())),
        Err(e) => (None, Some(e.to_string())),
    }
}

async fn parents(
    db: &DatabaseConnection,
    event: &app_events::Model,
) -> Result<Vec<Related>, DbErr> {
    let mut found = Vec::new();
    let mut next = event.causation_id;
    while let Some(id) = next.filter(|_| found.len() < MAX_PARENTS) {
        let Some(parent) = app_events::Entity::find_by_id(id)
            .filter(app_events::Column::OrgId.eq(event.org_id))
            .one(db)
            .await?
        else {
            break;
        };
        next = parent.causation_id;
        found.push(related(&parent));
    }
    Ok(found)
}

async fn children(
    db: &DatabaseConnection,
    event: &app_events::Model,
) -> Result<Vec<Related>, DbErr> {
    Ok(app_events::Entity::find()
        .filter(app_events::Column::OrgId.eq(event.org_id))
        .filter(app_events::Column::CausationId.eq(event.id))
        .order_by_asc(app_events::Column::PublishedAt)
        .limit(MAX_CHILDREN)
        .all(db)
        .await?
        .iter()
        .map(related)
        .collect())
}

fn related(e: &app_events::Model) -> Related {
    Related {
        id: e.id,
        event_type: e.r#type.clone(),
        key: e.key.clone(),
        published_at: e.published_at.with_timezone(&Utc),
    }
}

/// The deliveries of `events`, by event, each with its attempts when
/// `with_attempts`.
async fn delivery_views(
    db: &DatabaseConnection,
    events: &[Uuid],
    with_attempts: bool,
) -> Result<HashMap<Uuid, Vec<DeliveryView>>, DbErr> {
    let rows = deliveries::Entity::find()
        .filter(deliveries::Column::EventId.is_in(events.to_vec()))
        .order_by_asc(deliveries::Column::FunctionName)
        .all(db)
        .await?;
    let slugs = slugs(db, rows.iter().map(|d| d.app_id).collect()).await?;
    let mut tried = if with_attempts {
        attempts_of(db, rows.iter().map(|d| d.id).collect()).await?
    } else {
        HashMap::new()
    };
    let mut views: HashMap<Uuid, Vec<DeliveryView>> = HashMap::new();
    for d in rows {
        let app = slugs
            .get(&d.app_id)
            .cloned()
            .unwrap_or_else(|| d.app_id.to_string());
        let attempts = with_attempts.then(|| tried.remove(&d.id).unwrap_or_default());
        views.entry(d.event_id).or_default().push(DeliveryView {
            id: d.id,
            listener: format!("{app}/{}", d.function_name),
            environment: d.environment,
            status: d.status,
            attempt_count: d.attempts,
            last_error: d.last_error,
            run_id: d.run_id,
            created_at: d.created_at.with_timezone(&Utc),
            updated_at: d.updated_at.with_timezone(&Utc),
            attempts,
        });
    }
    Ok(views)
}

/// The attempts of `deliveries`, by delivery, oldest first, each with the
/// invocation it became.
pub(super) async fn attempts_of(
    db: &DatabaseConnection,
    deliveries: Vec<Uuid>,
) -> Result<HashMap<Uuid, Vec<AttemptView>>, DbErr> {
    let rows = attempts::Entity::find()
        .filter(attempts::Column::DeliveryId.is_in(deliveries))
        .order_by_asc(attempts::Column::Attempt)
        .all(db)
        .await?;
    let ran: Vec<Uuid> = rows.iter().filter_map(|a| a.invocation_id).collect();
    let invoked: HashMap<Uuid, InvocationView> = invocations::Entity::find()
        .filter(invocations::Column::Id.is_in(ran))
        .all(db)
        .await?
        .into_iter()
        .map(|i| (i.id, invocation_view(i)))
        .collect();
    let mut by_delivery: HashMap<Uuid, Vec<AttemptView>> = HashMap::new();
    for a in rows {
        by_delivery
            .entry(a.delivery_id)
            .or_default()
            .push(AttemptView {
                attempt: a.attempt,
                status: a.outcome.clone().unwrap_or_else(|| "running".to_string()),
                invocation: a.invocation_id.and_then(|id| invoked.get(&id).cloned()),
                outcome: a.outcome,
                started_at: a.started_at.with_timezone(&Utc),
                finished_at: a.finished_at.map(|t| t.with_timezone(&Utc)),
                error: a.error,
                invocation_id: a.invocation_id,
            });
    }
    Ok(by_delivery)
}

fn invocation_view(i: invocations::Model) -> InvocationView {
    InvocationView {
        id: i.id,
        status: i.status,
        duration_ms: i.duration_ms,
        result_status: i.result_status,
        build_id: i.build_id,
        created_at: i.created_at.with_timezone(&Utc),
    }
}

/// The slugs of `apps`.
pub(super) async fn slugs(
    db: &DatabaseConnection,
    ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, String>, DbErr> {
    Ok(apps::Entity::find()
        .filter(apps::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a.slug))
        .collect())
}
