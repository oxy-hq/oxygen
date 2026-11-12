//! Peer-cohort comparison's shared half: the wire contract and the pure
//! helpers both cohort surfaces answer with.
//!
//! The IDE's `semantic/cohort` and the customer-app twin differ only in how a
//! request gets in, where the semantic model comes from, and whether the
//! population is scoped to the viewer's reach. The request and response
//! shapes, which statistic a string names, and which cohort a measure
//! defaults to are facts about the model, so they live here, once.

pub mod resolve;
pub mod wire;

pub use resolve::{COHORT_TIMEOUT, parse_statistic, prepare_cohort_layer, resolve_cohort_name};
pub use wire::{CohortRequest, CohortResponse, CohortSubjectDto, ExcludedSubjectDto};
