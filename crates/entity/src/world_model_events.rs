//! `world_model_events` — the durable, cross-pod carrier for the world-model
//! live feed (order ripples, camera transitions, compliance reports).
//!
//! The in-process broadcast bus in `world_model.rs` is per-pod, so a webhook
//! landing on a `serve` replica never reached a viewer whose SSE connection
//! lives on the ide. Publishers now append a row here; every pod tails the
//! table by `id` and fans each row out onto its own local bus.
//!
//! Being a table rather than a notification channel buys three things at once:
//! cross-pod fan-out, history for a viewer that connects mid-shift, and a
//! countable `orders/min` (a stream can't be counted — reconnects lose events).
//!
//! Rows are disposable: a reaper trims anything past the retention window.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "world_model_events")]
pub struct Model {
    /// Monotonic cursor. Every pod's tailer remembers the last `id` it fanned
    /// out and asks only for rows above it, so the poll is an index range scan
    /// regardless of how big the retained window is.
    #[sea_orm(primary_key)]
    pub id: i64,
    pub workspace_id: Uuid,
    /// The already-serialised `WorldModelEvent`. Stored as JSON rather than a
    /// typed column because the SSE layer re-emits it verbatim — nothing on the
    /// read path needs to understand the shape, so nothing should have to be
    /// kept in sync with it.
    pub payload: Json,
    pub created_at: DateTimeWithTimeZone,
}

impl ActiveModelBehavior for ActiveModel {}
