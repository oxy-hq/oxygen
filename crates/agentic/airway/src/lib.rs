//! Airway ELT runtime — Pattern B subsystem on `agentic-runtime`.
//!
//! Wraps the external [airway] engine as a queue-driven ELT pipeline:
//! `SOURCE_TYPE` + `event_handler()` plug into the runtime's
//! `EventRegistry`; `AirwayWorker` runs a pipeline spec end-to-end;
//! `source_factory`/`destination_factory` dispatch the YAML `kind:` to
//! a concrete airway connector; and the `extension` module owns the
//! `airway_*` aggregates behind `AirwayMigrator`.
//!
//! [airway]: https://github.com/oxy-hq/airway-internal

pub mod admission;
pub mod boxed;
pub mod config;
pub mod contract;
pub mod deployment_config;
pub mod destination_factory;
pub mod error;
pub mod events;
pub mod extension;
pub mod placeholder;
pub mod preview;
pub mod report_validation;
pub mod reset;
pub mod schema_compat;
pub mod source_factory;
pub mod state_store;
pub mod task_spec;
pub mod upload_zone;
pub mod worker;

pub use admission::AirwayAdmission;
pub use config::{
    AirwayPipelineSpec, DestinationConfig, DestinationRef, DestinationSpec, SourceConfig,
};
pub use contract::{ContractMutability, ResourceContract, project_contracts};
/// The deployment (operational) tier — airway's process-wide `GlobalConfig`,
/// stored in the singleton `airway_deployment_config` row. Re-exported for the
/// same reason [`ContractPolicy`] is: oxy's staff admin surface has to
/// *name* these values (configured vs installed, and the drift between them)
/// and this crate is the boundary every other oxy crate enters airway through.
pub use deployment_config::{DeploymentValues, drift as deployment_drift, installed_values};
pub use destination_factory::build_destination;
pub use error::AirwayError;
pub use events::AirwayEvent;
pub use extension::AirwayMigrator;
pub use source_factory::{
    DiscoveredColumn, DiscoveredTable, build_source_connector, discover_source_tables,
};

/// Airway's admission vocabulary, re-exported for the same reason
/// [`DiscoveredTable`] is: a host that needs to *name* these types — oxy's
/// staff policy-preview endpoint reads `contracts()` / `resources()` off
/// a built connector and scores them against a [`ContractPolicy`] — should not
/// have to take a direct dependency on the `airway` engine to do it. This crate
/// is the boundary every other oxy crate enters airway through; widening it here
/// keeps that true.
pub use airway::connector::{
    ContractPolicy, Environment, ExtractionResult, Mutability, ResourceInfo, SourceConnector,
    SourceContract, admit_with,
};
pub use airway::types::WriteDisposition;

/// The **engine's** error type, aliased because it is not this crate's
/// [`AirwayError`] — `crate::error::AirwayError` wraps it, and having both in
/// scope under one name is exactly the confusion this alias avoids. Needed by
/// anything implementing [`SourceConnector`] outside this crate.
pub use airway::AirwayError as EngineError;
pub use state_store::{AirwayPgStateStore, AirwayRunScopedStateStore};
pub use worker::AirwayWorker;

/// QuickBooks refresh-token write-back port. The host (via
/// `agentic-pipeline`'s executor) supplies an implementation that
/// persists the rotated token to its secret store. This is a thin,
/// `String`-error port so callers don't need to depend on the `airway`
/// crate's error type — the factory bridges it to airway's own
/// `RefreshTokenSink` internally.
pub use source_factory::RefreshTokenSink;

/// QuickBooks **read-only** token port and the custody selector pairing it with
/// [`RefreshTokenSink`]. A grant tolerates exactly one rotation writer; when the
/// host already runs one, the pipeline supplies an `AccessTokenSource` instead
/// of a sink and never contacts Intuit's token endpoint.
pub use source_factory::{AccessTokenSource, QuickBooksTokens};

/// Host-side credential provider for `airhouse_managed` destinations. A thin,
/// `String`-error port (the factory bridges it to airway's `CredentialProvider`)
/// so an airway load re-mints a fresh ephemeral credential on every (re)connect
/// instead of reusing a possibly-expired static DSN.
pub use destination_factory::CredentialProvider;

/// `source_type` to register this domain under in the runtime event
/// registry. Used by the SSE layer to look up the right processor for a
/// run's events.
pub const SOURCE_TYPE: &str = "airway";

/// Build a [`DomainHandler`] for registering airway events with the
/// runtime's [`EventRegistry`].
///
/// Airway events are emitted as `(event_type, payload)` JSON pairs by
/// the worker; the processor is a passthrough that preserves both
/// fields verbatim. Same shape as `agentic-automation`'s event handler.
///
/// [`DomainHandler`]: agentic_runtime::event_registry::DomainHandler
/// [`EventRegistry`]: agentic_runtime::event_registry::EventRegistry
pub fn event_handler() -> agentic_runtime::event_registry::DomainHandler {
    use agentic_runtime::event_registry::{DomainHandler, RowProcessor};
    use std::sync::Arc;

    let processor: RowProcessor =
        Arc::new(|event_type, payload| Some(vec![(event_type.to_string(), payload.clone())]));

    DomainHandler {
        processor,
        summary_fn: Arc::new(|_| None),
        tool_summary_fn: Arc::new(|_, _| None),
        should_accumulate: Some(Arc::new(|_| false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_type_is_airway() {
        assert_eq!(SOURCE_TYPE, "airway");
    }

    #[test]
    fn event_handler_constructs() {
        // Smoke test — confirms the handler builds without panicking.
        let _ = event_handler();
    }
}
