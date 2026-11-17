//! `simulation_runs` — one row per run of a declared world.
//!
//! `spec` is a snapshot rather than a pointer at the `.simulation.yml`: a run is
//! evidence, and one that could only be read alongside the current file would
//! silently re-interpret itself the next time the world was retuned.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// No `Eq`: `ModelEx` holds the run's periods and fits, and both of those
// models carry floats.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "simulation_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: Uuid,
    pub workspace_id: Uuid,
    pub revision_id: Option<Uuid>,
    pub simulation_name: String,
    /// The arm this run is — `hold` | `legacy` | `machine` | `machine_explore`
    /// | `oracle`. Chosen when the run was queued, not read off the world: the
    /// arms of a profit race have to be runs of ONE world or the comparison is
    /// between two worlds that happen to look alike.
    pub policy: String,
    /// Stored as `big_integer` because Postgres has no unsigned type; the
    /// store writes `spec.seed as i64`, a lossless bit-cast. The wire form
    /// undoes the cast, so a seed above `i64::MAX` reads back as the `u64` it
    /// was declared as rather than as a negative number.
    #[serde(serialize_with = "seed_as_u64", deserialize_with = "seed_from_wire")]
    pub seed: i64,
    /// Which draw of the world this is, `0` for the seed the file declares.
    ///
    /// Runs of the same `(simulation_name, policy)` across replicates are the
    /// same experiment repeated — a cell of the outcome map is their aggregate,
    /// never a single one of them.
    pub replicate: i32,
    /// `queued` | `running` | `done` | `failed` | `cancelled`.
    pub status: String,
    pub spec: Json,
    /// The world's solved true parameters. The one place truth is allowed to
    /// land — written by the scorer, never read back into the loop.
    pub truth: Option<Json>,
    pub periods_planned: i32,
    pub periods_done: i32,
    /// When the run was enqueued — the HTTP handler's clock.
    pub queued_at: DateTimeWithTimeZone,
    /// When a worker claimed the run. Equal to `queued_at` until one does, so
    /// the column can stay `NOT NULL` (the listing index orders on it) and a
    /// run still waiting shows a zero-length runtime rather than a null.
    pub started_at: DateTimeWithTimeZone,
    pub finished_at: Option<DateTimeWithTimeZone>,
    pub error: Option<String>,
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub simulation_run_periods: HasMany<super::simulation_run_periods::Entity>,
    #[sea_orm(has_many)]
    #[serde(skip)]
    pub simulation_run_fits: HasMany<super::simulation_run_fits::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

fn seed_as_u64<S: Serializer>(seed: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(*seed as u64)
}

/// Accepts both spellings: the `u64` this module writes, and the raw `i64` any
/// row serialised before the cast was undone.
fn seed_from_wire<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seed {
        Unsigned(u64),
        Signed(i64),
    }
    Ok(match Seed::deserialize(d)? {
        Seed::Unsigned(v) => v as i64,
        Seed::Signed(v) => v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(seed: i64) -> Model {
        let now = chrono::Utc::now().fixed_offset();
        Model {
            run_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            revision_id: None,
            simulation_name: "w".into(),
            policy: "machine".into(),
            seed,
            replicate: 0,
            status: "queued".into(),
            spec: serde_json::json!({}),
            truth: None,
            periods_planned: 1,
            periods_done: 0,
            queued_at: now,
            started_at: now,
            finished_at: None,
            error: None,
        }
    }

    /// The column is `big_integer` and the store writes `spec.seed as i64` — a
    /// lossless bit-cast. Reading it back has to undo the cast, or a seed above
    /// `i64::MAX` reads as negative on GET while POST's `EnqueuedRun.seed` was
    /// right.
    #[test]
    fn seed_serialises_as_the_u64_it_was_cast_from() {
        let json = serde_json::to_value(model(-1)).expect("serialise");
        assert_eq!(
            json["seed"],
            serde_json::json!(18_446_744_073_709_551_615_u64)
        );
    }

    /// And the wire form reads back to the same row — the bit-cast in both
    /// directions.
    #[test]
    fn seed_round_trips_through_the_wire_form() {
        let json = serde_json::to_value(model(-1)).expect("serialise");
        let back: Model = serde_json::from_value(json).expect("deserialise");
        assert_eq!(back.seed, -1);
        // A seed that fits in i64 is unchanged either way.
        let json = serde_json::to_value(model(7)).expect("serialise");
        assert_eq!(json["seed"], serde_json::json!(7));
    }
}
