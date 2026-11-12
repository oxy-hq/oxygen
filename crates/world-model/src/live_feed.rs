//! The world-model live feed's write half: the events, and the outbox that
//! appends them to `world_model_events`.
//!
//! Publishers live on both sides of `oxy-app`'s line — the Toast webhook and the
//! UniFi watcher inside it, the camera-domain bridge in `oxy-api-world-model` —
//! so this half sits below both. It never touches a bus: a publish is a row, and
//! every pod's tailer (in `oxy-api-world-model`, with the per-workspace buses
//! and the SSE route) fans the row back out. Why the feed goes through Postgres
//! at all is that crate's `events` module doc.

use chrono::{DateTime, Utc};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;
use uuid::Uuid;

/// Camera state-change event (from the UniFi watcher). Surfaces in the
/// CAMERAS tab of LIVE EVENTS — independent of `OrderEvent` so the FE can
/// distinguish them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraStateEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub camera_id: String,
    pub host_name: String,
    pub status: String,
}

/// Compliance report ingested from the edge worker. `type` is
/// `compliance_violation` when the verdict is non-compliant, else
/// `compliance_report` — alert on the former, tick a live detection
/// counter on the latter.
#[derive(Debug, Clone, Serialize)]
pub struct ComplianceEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub camera_id: String,
    pub report_id: String,
    pub violation: bool,
    pub missing_items: Vec<String>,
    pub confidence: Option<f64>,
    pub segment_start: DateTime<Utc>,
    pub segment_end: DateTime<Utc>,
    pub ts: DateTime<Utc>,
}

/// Camera health transition from the alerter tick (`ok`/`degraded`/`stale`).
#[derive(Debug, Clone, Serialize)]
pub struct CameraHealthEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub camera_id: String,
    pub from: String,
    pub to: String,
    pub reason: String,
    pub ts: DateTime<Utc>,
}

/// Tagged union the SSE channel actually broadcasts. Lets one channel carry
/// both order ripples and camera transitions without a second handler.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum WorldModelEvent {
    Order(OrderEvent),
    Camera(CameraStateEvent),
    Compliance(ComplianceEvent),
    Health(CameraHealthEvent),
}

/// One ripple's worth of data — what the world-model app needs to draw a
/// concentric ring + append a row to LIVE EVENTS. Mirrors the FE union arm
/// in `web-app/src/pages/world-model/types.ts` (`order_ripple`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OrderEvent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub key: String,
    pub store_name: Option<String>,
    pub amount: f64,
    pub order_id: String,
    pub ts: DateTime<Utc>,
    /// The org location `key` resolves to through `location_external_ids`
    /// (system `toast`), when it is mapped. Optional on both sides: rows
    /// published before the operating graph carry neither, and an unmapped
    /// store is still an order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_name: Option<String>,
}

/// Queue of events waiting to be appended to `world_model_events`.
///
/// Publishers are synchronous — the cameras crate's sink is a plain
/// `Fn(Uuid, CameraDomainEvent)`, and the Toast handler publishes on the
/// response path — so they hand off here instead of awaiting a write. Bounded:
/// if the writer ever falls behind, dropping a live-feed frame is the correct
/// failure, not stalling a webhook that Toast will retry.
fn outbox() -> tokio::sync::mpsc::Sender<(Uuid, Value)> {
    static OUTBOX: OnceLock<tokio::sync::mpsc::Sender<(Uuid, Value)>> = OnceLock::new();
    OUTBOX
        .get_or_init(|| {
            let (tx, rx) = tokio::sync::mpsc::channel(4096);
            spawn_writer(rx);
            tx
        })
        .clone()
}

/// Hand an event to the outbox. Never blocks, never fails the caller.
pub fn publish(workspace_id: Uuid, event: WorldModelEvent) {
    let value = match serde_json::to_value(&event) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "world-model: event failed to serialise; dropped");
            return;
        }
    };
    if outbox().try_send((workspace_id, value)).is_err() {
        tracing::warn!(
            workspace = %workspace_id,
            "world-model outbox full; dropping a live-feed event (the dashboard \
             will be missing a frame, nothing durable is lost)"
        );
    }
}

/// Webhook-side entry point — called by the Toast receiver after it has
/// validated the payload, scoped to the webhook's workspace.
pub fn publish_order(workspace_id: Uuid, event: OrderEvent) {
    publish(workspace_id, WorldModelEvent::Order(event));
}

/// Camera-watcher entry point, scoped to the watcher's workspace.
pub fn publish_camera_event(workspace_id: Uuid, event: CameraStateEvent) {
    publish(workspace_id, WorldModelEvent::Camera(event));
}

/// Drains the outbox into `world_model_events`.
///
/// Batched: under a burst this collapses many single-row inserts into one
/// statement, which is what keeps the hot path off the webhook's response time.
/// A write failure is logged and the batch dropped — the feed is disposable by
/// design, and blocking or retrying forever would turn a cosmetic outage into a
/// backpressure one.
fn spawn_writer(mut rx: tokio::sync::mpsc::Receiver<(Uuid, Value)>) {
    tokio::spawn(async move {
        let mut batch: Vec<(Uuid, Value)> = Vec::with_capacity(64);
        loop {
            let Some(first) = rx.recv().await else {
                return; // sender dropped — process is shutting down
            };
            batch.push(first);
            while let Ok(next) = rx.try_recv() {
                batch.push(next);
                if batch.len() >= 256 {
                    break;
                }
            }

            let db = match oxy::database::client::establish_connection().await {
                Ok(db) => db,
                Err(e) => {
                    tracing::warn!(error = %e, dropped = batch.len(),
                        "world-model writer: no database; dropping events");
                    batch.clear();
                    continue;
                }
            };

            let rows: Vec<entity::world_model_events::ActiveModel> = batch
                .drain(..)
                .map(
                    |(workspace_id, payload)| entity::world_model_events::ActiveModel {
                        workspace_id: sea_orm::ActiveValue::Set(workspace_id),
                        payload: sea_orm::ActiveValue::Set(payload),
                        ..Default::default()
                    },
                )
                .collect();
            let n = rows.len();
            if let Err(e) = entity::world_model_events::Entity::insert_many(rows)
                .exec(&db)
                .await
            {
                tracing::warn!(error = %e, dropped = n, "world-model writer: insert failed");
            }
        }
    });
}
