//! Workspace-preview Airway samples: a bounded window of a previewed branch's
//! pipeline, run under a preview-owned pipeline key (phase 2b, D3/P4).
//!
//! Two halves, both pure:
//!
//! * [`SamplePolicy`] — at submit. Which sources may be sampled at all, and what
//!   a sample of each must say: a date window for the windowed kinds
//!   ([`WINDOWED_BACKFILL_KINDS`]), named resources for the others (which run
//!   under a wall-clock cap instead), and a registered sandbox for the
//!   rotate-on-use kinds.
//! * [`PreviewSample::apply`] — at claim, on the parsed spec, before anything
//!   reads it. It renames the pipeline `preview:<key>:<name>`, so everything
//!   keyed by the name — the single-flight lease, the cursor and stored schema,
//!   the load audit, the run extension — is the preview's own and production's
//!   `(workspace, name)` rows are never read or written. It also makes the run
//!   single-flight, drops a `schema_separator` (tables stay in the one dataset
//!   the preview platform maps), and points a rotate-on-use source at the
//!   customer's sandbox company with the sandbox's own credential names, in the
//!   vendor's `sandbox` environment.
//!
//! The destination is the host's: the preview platform resolves the `database:`
//! reference into the preview's own Airhouse schemas with a Writer confined to
//! them. Nothing here names a credential, and nothing here can reach a source.

use crate::config::RESERVED_NAME_PREFIX;
pub use crate::task_spec::WINDOWED_BACKFILL_KINDS;

mod apply;
mod policy;
mod sandbox;
#[cfg(test)]
mod tests;

pub use apply::{AppliedSample, PreviewSample};
pub use policy::{
    RequestedWindow, SampleAsk, SamplePlan, SamplePolicy, SampleWindow, metadata_in_main,
};
pub use sandbox::SandboxSource;

/// Sources a preview never samples. `postgres_cdc` and `pgoutput` advance a
/// replication slot production shares; `sp_api` is quota-limited and a report
/// window cannot be pulled twice.
pub const SAMPLE_REFUSED_KINDS: [&str; 3] = ["postgres_cdc", "pgoutput", "sp_api"];

/// Sources whose credential rotates when it is used: Intuit voids a QuickBooks
/// refresh token when it issues the next one, so a second rotator bricks the
/// grant. A sample of one runs only against a registered sandbox company.
pub const ROTATE_ON_USE_KINDS: [&str; 1] = ["quickbooks"];

/// A windowed sample with no window asked for covers the last week.
pub const DEFAULT_WINDOW_DAYS: i64 = 7;
/// The longest window a sample may ask for.
pub const MAX_WINDOW_DAYS: i64 = 31;

/// `preview:<key>:<name>` — the name a sample of pipeline `name` runs under in
/// preview `key`. [`AirwayPipelineSpec::validate`] refuses the prefix in any
/// authored YAML, so no production pipeline can already be called this.
pub fn scoped_pipeline_name(preview_key: &str, name: &str) -> String {
    format!("{RESERVED_NAME_PREFIX}{preview_key}:{name}")
}

pub fn is_windowed(kind: &str) -> bool {
    WINDOWED_BACKFILL_KINDS.contains(&kind)
}

pub fn rotates_on_use(kind: &str) -> bool {
    ROTATE_ON_USE_KINDS.contains(&kind)
}

/// Why a sample was refused, with the contract's code for each.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SampleRefusal {
    #[error(
        "`{kind}` sources are never sampled in a preview: postgres_cdc and pgoutput advance a \
         replication slot production shares, and sp_api is quota-limited and cannot be pulled \
         twice"
    )]
    Refused { kind: String },
    #[error(
        "pipeline `{pipeline}` reads a source whose credential rotates on use; register a \
         sandbox company for it (PUT /previews/sources) before sampling it"
    )]
    SandboxRequired { pipeline: String },
    #[error("{0}")]
    WindowRequired(String),
    #[error("a sample window may cover at most {MAX_WINDOW_DAYS} days; this one covers {days}")]
    WindowTooLong { days: i64 },
    #[error("`{kind}` sources take no date window; name `resources` instead")]
    WindowNotSupported { kind: String },
    #[error(
        "this source advertises {} resources ({}); a sample of a source without a date window \
         must name the ones it reads in `resources`",
        advertised.len(), advertised.join(", ")
    )]
    ResourcesRequired { advertised: Vec<String> },
    #[error("`{name}` is not a resource this source advertises ({})", advertised.join(", "))]
    UnknownResource {
        name: String,
        advertised: Vec<String>,
    },
    #[error(
        "the pipeline sets an explicit `base_url`; a sample of a rotate-on-use source runs only \
         against the vendor's sandbox host, so it is refused rather than sent where the branch \
         points"
    )]
    ExplicitBaseUrl,
    #[error(
        "the pipeline's destination is an inline `{kind}` connection; a sample lands only in the \
         preview's own schemas, through a `database:` reference"
    )]
    InlineDestination { kind: String },
    #[error(
        "the pipeline lands in `{database}`, which is not the workspace's managed Airhouse; a \
         sample lands only in the preview's own schemas there"
    )]
    NotManagedAirhouse { database: String },
    #[error(
        "airway writes its own metadata tables in `main`, which a preview can't write yet: {} \
         load as `replacing` (or carry a watermark column), so sample other resources or wait \
         until airway can put that metadata elsewhere",
        tables.join(", ")
    )]
    Unsupported { tables: Vec<String> },
    #[error("{0}")]
    Invalid(String),
}

impl SampleRefusal {
    /// The contract's error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused { .. }
            | Self::ExplicitBaseUrl
            | Self::InlineDestination { .. }
            | Self::NotManagedAirhouse { .. } => "sample_refused",
            Self::SandboxRequired { .. } => "sandbox_required",
            Self::WindowRequired(_) => "window_required",
            Self::WindowTooLong { .. } => "window_too_long",
            Self::WindowNotSupported { .. } => "window_not_supported",
            Self::ResourcesRequired { .. } => "resources_required",
            Self::UnknownResource { .. } => "unknown_resource",
            Self::Unsupported { .. } => "sample_unsupported",
            Self::Invalid(_) => "bad_request",
        }
    }
}
