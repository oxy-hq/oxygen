//! `experiment_proposals` — one row per candidate design priced.
//!
//! The handler inserts one row per candidate `pending`, all sharing a
//! `request_id`, each holding its `Candidate`; the price job settles each
//! `done`, `refused` or `failed`, so a refusal or a crash is always a row and
//! never a timeout. A seed proves reproducibility, not that the assignment was
//! the first drawn (`internal-docs/experiments.md`, "Randomisation and the
//! proposal log"): the report counts the `done` rows written before the
//! registered one — candidates compared side by side included — which is what
//! makes a re-roll visible.
//!
//! `id` is also the queue task id. `seed` is `u64 as i64`; read it back with
//! `as u64`. `waves` is `[{ "switch_on": "YYYY-MM-DD", "units": [..] }, ..]`
//! (group names under `assign_by`); `periods` is `[{ "from": "YYYY-MM-DD",
//! "state": "on" | "off" }, ..]`; `power` is the engine's `PowerResult`, whole.
//! For a `failed` row `refusal` holds the reason. Within one `request_id`,
//! `created_at` ascends in candidate order.
//!
//! No `Eq` derive: the model carries floats.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "experiment_proposals")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub workspace_id: Uuid,
    pub name: String,
    /// The experiment's design. `None` only on a row logged before designs
    /// were records.
    pub design_id: Option<Uuid>,
    /// The design's `content_hash` when this was priced: the draft is edited in
    /// place, so a different hash now means this priced an earlier draft.
    pub design_hash: Option<String>,
    /// The price request this candidate came in.
    pub request_id: Uuid,
    /// `pending` | `done` | `refused` | `failed`.
    pub status: String,
    /// The one `Candidate`, as received (index C5).
    pub request: Json,
    /// The α this candidate is priced at: its override, else `outcome.alpha`.
    pub alpha: f64,
    pub seed: Option<i64>,
    pub waves: Option<Json>,
    pub periods: Option<Json>,
    /// `string[][]`, the blocks the assignment was randomised within.
    pub strata: Option<Json>,
    pub assign_by: Option<String>,
    pub power: Option<Json>,
    /// `power.mde` in $ per unit-day (index C2b).
    pub mde_unit_day: Option<f64>,
    /// `{ "value": f64, "source": "declared" | "fitted" }`, frozen at pricing.
    pub prior_coefficient: Option<Json>,
    /// `["…", …]`
    pub warnings: Json,
    pub refusal: Option<String>,
    pub requested_by: Uuid,
    pub finished_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
