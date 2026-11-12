//! World-model live event channel: the read half.
//!
//! The events and the outbox that writes them are `oxy_world_model::live_feed`,
//! below `oxy-app` so its publishers reach them; this module is the bus, the
//! tailer that fills it, the reaper, and the SSE route that drains it.
//!
//! One `broadcast` sender per workspace, keyed by `workspace_id` in a
//! `DashMap`. The SSE endpoint subscribes to only its own workspace's bus.
//! Keying by workspace prevents an authenticated subscriber on one workspace
//! from receiving another workspace's order ripples / camera events.
//!
//! ## Why publishing goes through Postgres
//!
//! The bus is **per process**. `POST /api/webhooks/toast/orders` is `FleetOk`
//! and runs on a `serve` replica; `GET /api/{ws}/world-model/events` was
//! `IdeOnly` and proxied to the ide. Publisher and subscriber were therefore
//! never in the same process, and every order ripple was dropped on the floor —
//! not "roughly one in three", but all of them. #2816 changed the webhook's
//! answer from `401` to `202`; the event still went nowhere.
//!
//! The subscriber side is `FleetOk` now, which it could only become once the
//! feed below stopped being process-local. That is a consequence of the fix,
//! not a second one: with the table in place any replica can serve a viewer,
//! and pinning the panel to the singleton bought nothing.
//!
//! Toast is the loudest case, not the only one. Both camera producers are
//! fleet-side too: `ComplianceReport` is emitted from the edge ingest handler
//! (`POST /control/compliance-reports` → `cameras::service::compliance::
//! write_reports`), and that whole `/control` tree is merged as a plain
//! `Router`, so it carries no role declaration and `classify` falls through to
//! its `FleetOk` default. Only `HealthTransition` reached a subscriber before
//! this change, and only by accident: `alerts::spawn` is started on every pod,
//! so the ide's own copy of the loop published into the ide's own bus. So do
//! not read the camera tab having looked half-alive as evidence that cameras
//! were fine.
//!
//! So publishers no longer touch the bus directly. They append a row to
//! `world_model_events`, and every pod tails that table and fans each row onto
//! its own local bus. Fan-out is the same single path everywhere, including on
//! the publishing pod, so no pod is a special case.
//!
//! One subscriber-side overlap survives that: a viewer subscribes *before*
//! reading its backfill (so nothing published in between is lost), which means
//! the receiver can hold rows the backfill also returned. Every event therefore
//! travels the bus as a [`LiveEvent`] carrying its row id, and the stream drops
//! anything at or below the last id the backfill already sent. Without it a
//! reconnect can show one order twice and count it twice in `orders/min`.
//!
//! A table rather than `LISTEN`/`NOTIFY` on purpose. The notify path costs a
//! second permanent LISTEN connection per pod, which
//! `internal-docs/adr-postgres-as-worker-queue.md` already flags as a
//! horizontal-scale cap (§3) and a PgBouncer footgun (§1) — and it would still
//! leave a viewer who connects mid-shift with an empty panel and `orders/min`
//! uncountable. Reading rows solves all three with no new connection.

use axum::extract::Path;
use axum::response::sse::{KeepAlive, Sse};
use chrono::Utc;
use dashmap::DashMap;
use futures::StreamExt;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;
use tokio::sync::broadcast;
use uuid::Uuid;

use oxy_app::surface::WorkspaceExtractor;

/// How long a published event stays readable. Long enough that a viewer opening
/// the dashboard mid-shift sees a populated panel; short enough that the table
/// stays small and the reaper's delete is cheap.
const RETENTION: chrono::Duration = chrono::Duration::hours(6);

/// How many past events a newly-connected subscriber receives before the live
/// stream takes over.
const BACKFILL_LIMIT: u64 = 100;

/// Tailer poll interval. Sub-second, so "LIVE" is honest, and it is one small
/// indexed range scan per pod — independent of how many viewers are connected.
const POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// One event as it travels this pod's bus.
///
/// `payload` is the JSON the client receives, read straight out of the row —
/// nothing on the read path deserialises back into a typed enum it would only
/// re-serialise a moment later. `id` is the row id, and it rides along **only**
/// so a subscriber can tell a live frame apart from one its backfill already
/// covered; it is deliberately not part of the wire format.
#[derive(Clone, Debug)]
pub struct LiveEvent {
    pub id: i64,
    pub payload: Value,
}

/// Per-workspace broadcast buses. Each sender has capacity 1024 — well above
/// the chain's order velocity, so slow SSE consumers won't drop ripples in
/// normal operation. Entries are created lazily on first publish/subscribe and
/// never removed (a `Sender` is tiny, and the workspace set is bounded).
fn buses() -> &'static DashMap<Uuid, broadcast::Sender<LiveEvent>> {
    static BUSES: OnceLock<DashMap<Uuid, broadcast::Sender<LiveEvent>>> = OnceLock::new();
    BUSES.get_or_init(DashMap::new)
}

/// Get (or create) the broadcast sender for one workspace.
fn bus_for(workspace_id: Uuid) -> broadcast::Sender<LiveEvent> {
    buses()
        .entry(workspace_id)
        .or_insert_with(|| broadcast::channel::<LiveEvent>(1024).0)
        .clone()
}

/// Subscribe to one workspace's live feed on **this** pod.
///
/// Events arrive here via the tailer, so a subscriber sees what every other pod
/// published as well as what this one did — that is the whole point of routing
/// publishes through the table.
pub fn subscribe(workspace_id: Uuid) -> broadcast::Receiver<LiveEvent> {
    bus_for(workspace_id).subscribe()
}

/// Tails `world_model_events` and fans new rows onto this pod's buses.
///
/// Runs on **every** pod, including the one that published — that is what makes
/// the delivery path identical everywhere and keeps a subscriber from seeing an
/// event twice.
///
/// The cursor starts at the current MAX(id) rather than 0: on restart we want
/// the live feed, not a replay of the retained window into every open tab.
/// A viewer that wants history gets it from the backfill on connect instead.
pub fn spawn_world_model_tailer() {
    // One tailer per process. A second one would fan every row onto the same
    // bus again, so a subscriber would see each event as many times as this was
    // called — the background seam starts it once per process, and this makes that a property
    // of the function rather than of its one call site.
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut cursor: i64 = -1;
        loop {
            tokio::time::sleep(POLL).await;

            let Ok(db) = oxy::database::client::establish_connection().await else {
                continue; // transient; try again next tick
            };

            // First successful connection sets the high-water mark.
            if cursor < 0 {
                cursor = entity::world_model_events::Entity::find()
                    .order_by_desc(entity::world_model_events::Column::Id)
                    .one(&db)
                    .await
                    .ok()
                    .flatten()
                    .map(|m| m.id)
                    .unwrap_or(0);
                continue;
            }

            let rows = match entity::world_model_events::Entity::find()
                .filter(entity::world_model_events::Column::Id.gt(cursor))
                .order_by_asc(entity::world_model_events::Column::Id)
                .limit(1000)
                .all(&db)
                .await
            {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::debug!(error = %e, "world-model tailer: poll failed");
                    continue;
                }
            };

            for row in rows {
                cursor = row.id;
                // `send` errors only when nobody is subscribed on this pod,
                // which is the normal case for most pods most of the time.
                let _ = bus_for(row.workspace_id).send(LiveEvent {
                    id: row.id,
                    payload: row.payload,
                });
            }
        }
    });
}

/// Trims events past `RETENTION`. Cheap and idempotent, so it is safe to run
/// on every pod; whoever gets there first does the work.
pub fn spawn_world_model_reaper() {
    // Idempotent for the same reason as the tailer; duplicate reapers would only
    // race each other to delete the same rows.
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(600));
        loop {
            tick.tick().await;
            let Ok(db) = oxy::database::client::establish_connection().await else {
                continue;
            };
            let cutoff = Utc::now() - RETENTION;
            if let Err(e) = entity::world_model_events::Entity::delete_many()
                .filter(entity::world_model_events::Column::CreatedAt.lt(cutoff))
                .exec(&db)
                .await
            {
                tracing::debug!(error = %e, "world-model reaper: delete failed");
            }
        }
    });
}

/// The most recent events for one workspace, oldest-first, for the backfill a
/// newly-connected subscriber receives before the live stream takes over.
///
/// Returns the highest row id it covered alongside them (`0` when there is no
/// history yet). That id is the boundary [`live_events`] filters against, so it
/// has to come from the same read as the rows — recomputing it later would
/// reopen the very gap it exists to close.
///
/// `pub` alongside [`live_events`], for the same test.
pub async fn recent_events(workspace_id: Uuid) -> (Vec<LiveEvent>, i64) {
    let Ok(db) = oxy::database::client::establish_connection().await else {
        return (Vec::new(), 0);
    };
    let mut rows = entity::world_model_events::Entity::find()
        .filter(entity::world_model_events::Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(entity::world_model_events::Column::Id)
        .limit(BACKFILL_LIMIT)
        .all(&db)
        .await
        .unwrap_or_default();
    rows.reverse();
    // `rows` is oldest-first now, so the newest id is the last one.
    let through_id = rows.last().map(|r| r.id).unwrap_or(0);
    let events = rows
        .into_iter()
        .map(|r| LiveEvent {
            id: r.id,
            payload: r.payload,
        })
        .collect();
    (events, through_id)
}

/// The backfill, then the live feed with everything the backfill already
/// covered filtered out.
///
/// The handler subscribes before it reads the backfill, so nothing published in
/// between is lost — but that same overlap lets the receiver hold rows the
/// backfill also returned. Dropping `id <= through_id` is what keeps a viewer
/// from seeing one order twice and counting it twice in `orders/min`.
///
/// `pub` for the `world_model_cross_pod` integration test, which drives the
/// overlap directly — the handler above it needs a `WorkspaceExtractor` and a
/// live axum stack to reach, and an `Sse` frame exposes no way to read its data
/// back.
pub fn live_events(
    backfill: Vec<LiveEvent>,
    mut receiver: broadcast::Receiver<LiveEvent>,
    through_id: i64,
) -> impl futures::Stream<Item = LiveEvent> {
    async_stream::stream! {
        for event in backfill {
            yield event;
        }
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    if event.id <= through_id {
                        // Published between our subscribe and our backfill read;
                        // the backfill already sent it.
                        continue;
                    }
                    yield event;
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "world-model SSE receiver stopped");
                    break;
                }
            }
        }
    }
}

/// [`live_events`] as SSE frames.
///
/// Only `payload` goes on the wire, so a client sees byte-for-byte what it saw
/// before the id started riding along. `Value::to_string` cannot fail, so unlike
/// the generic `create_sse_broadcast` there is no serialization-error branch.
fn live_stream(
    backfill: Vec<LiveEvent>,
    receiver: broadcast::Receiver<LiveEvent>,
    through_id: i64,
) -> impl futures::Stream<Item = Result<axum::response::sse::Event, axum::Error>> {
    live_events(backfill, receiver, through_id).map(|event| {
        Ok(axum::response::sse::Event::default()
            .event("message")
            .data(event.payload.to_string()))
    })
}

#[derive(Deserialize)]
pub struct WorkspacePath {
    pub workspace_id: Uuid,
}

/// `GET /api/{workspace_id}/world-model/events`
///
/// SSE stream that fans the broadcast bus out to one browser tab, after a
/// backfill of recent history. Uses `live_stream` rather than the generic
/// `create_sse_broadcast` because the two halves overlap and the duplicates
/// have to be filtered; the frames on the wire are identical either way.
///
/// Authentication: standard `X-API-Key` header — also accepted as a
/// `?api_key=…` query param via the `api_key_query` middleware (browsers
/// can't attach headers to `EventSource` requests).
#[axum::debug_handler]
#[tracing::instrument(skip_all)]
pub async fn world_model_events_sse(
    _workspace: WorkspaceExtractor,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, axum::Error>>> {
    tracing::info!(workspace = %workspace_id, "world-model SSE subscriber connected");
    // Subscribe BEFORE reading history so an event published between the two is
    // delivered by the stream rather than lost in the gap. That trades the gap
    // for an overlap, which `live_stream` closes with `through_id`.
    let receiver = subscribe(workspace_id);
    let (backfill, through_id) = recent_events(workspace_id).await;
    Sse::new(live_stream(backfill, receiver, through_id))
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}
