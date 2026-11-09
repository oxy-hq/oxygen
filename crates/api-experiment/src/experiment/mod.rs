//! Experiments — the host half, pricing (`internal-docs/experiments.md`).
//!
//! `oxy-experiment` owns the file, its refusals and the calendar; airlayer's
//! `engine::experiment` owns the statistics. This module is the seam between
//! them and Postgres: it reads the compiled definition, asks the warehouse for
//! the panel, hands both to the engine, and settles a proposal row with what
//! came back.
//!
//! * [`assignment`] — config → the engine's `ExperimentDesign`.
//! * [`candidate`] — the price request's candidate designs.
//! * [`panel`] / [`panel_groups`] — the one semantic query, its empty-result
//!   short-circuit, and the group-summed panel under `assign_by`.
//! * [`price`] / [`price_waves`] / [`price_switchback`] / [`snippet`] /
//!   [`request`] — pure pricing, the YAML the user commits, and the request body.
//! * [`lever`] / [`lag`] / [`spread`] / [`prior`] / [`reads`] — the lever
//!   check, the warnings the warehouse informs, the frozen slope, and the one
//!   read that gathers them.
//! * [`proposal_store`] — the only place these tables are read or written.
//! * [`job`] / [`executor`] / [`source`] — `TaskSpec::Custom { kind: "experiment_price" }`,
//!   registered exactly like `SimulationTaskExecutor`.
//! * [`enqueue`] / [`summary`] — what the HTTP handlers call.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod abort;
pub mod adopt;
pub mod apply;
pub mod assignment;
pub mod batch;
pub mod candidate;
pub mod check;
pub mod design_store;
pub mod dispatch;
pub mod enqueue;
pub mod estimate;
pub mod executor;
pub mod glance;
pub mod interim;
pub mod job;
pub mod jobs;
pub mod lag;
pub mod lever;
pub mod lever_refs;
pub mod list;
pub mod notify;
pub mod panel;
pub mod panel_groups;
pub mod panel_rows;
pub mod panel_spec;
pub(crate) mod panel_workspace;
pub mod price;
pub mod price_switchback;
pub mod price_waves;
pub mod prior;
pub mod proposal_store;
pub mod reads;
pub mod record;
pub mod refusal_note;
pub mod register;
pub mod reopen;
pub mod request;
pub mod results_api;
pub mod retrospective;
pub mod schedule;
mod send;
pub mod snippet;
pub mod source;
pub mod split_edit;
pub mod spread;
pub mod status;
pub mod status_flips;
pub mod store;
pub mod store_flips;
pub mod store_runs;
pub mod summary;
pub mod tick;

pub use executor::ExperimentPriceExecutor;

/// `TaskSpec::Custom` discriminator. Becomes the run's `source_type`.
pub const EXPERIMENT_PRICE_KIND: &str = "experiment_price";

/// What the queue carries (index C4): only the id of the `pending` proposal row
/// the handler inserted — one job per candidate. The candidate, the workspace,
/// the revision and the requester live on that row, so a refusal or a crash is
/// always visible as a row and never as a timeout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentPricePayload {
    pub proposal_id: Uuid,
}

#[cfg(test)]
mod panel_groups_tests;
#[cfg(test)]
mod panel_tests;
#[cfg(test)]
mod price_switchback_tests;
#[cfg(test)]
mod price_tests;
#[cfg(test)]
mod reads_tests;
#[cfg(test)]
pub(crate) mod test_design;
#[cfg(test)]
mod testing;

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn the_payload_is_exactly_the_c4_shape() {
        let p = ExperimentPricePayload {
            proposal_id: Uuid::from_u128(7),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["proposal_id"]
        );
        assert_eq!(
            serde_json::from_value::<ExperimentPricePayload>(v).unwrap(),
            p
        );
    }
}
