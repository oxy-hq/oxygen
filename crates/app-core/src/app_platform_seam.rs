//! What a custom app's functions may ask of the platform itself: the work
//! behind the host ops an org admin approved for the app
//! (`internal-docs/proposals/app-platform-access.md`).
//!
//! The function runtime (`oxy-api-custom-apps`) decides each call first: the
//! manifest asked for it, an org admin approved it, and `oxy-authz` allows it
//! for the app's service account. Then it asks this object to do the work. The
//! work belongs to other contexts (kiosk devices to `oxy-api-frontline`, the
//! landing zone to `oxy-api-airway`), and one API crate never imports another
//! (`internal-docs/domain-boundaries.md`, S3). So it is handed in: `oxy-server`
//! installs it at boot in every process that runs functions, as it installs
//! the notification transport (`crate::notifications::inline`).
//!
//! Nothing here decides access. Every method assumes the decision was made.
//! A process that never installs an implementation refuses every such op, and
//! says why.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

/// One kiosk-device operation, as a function asks for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontlineDevicesOp {
    /// Every device of the org, newest first.
    List,
    /// A new device, answered with its one-time enrol link.
    Create {
        name: String,
        location_id: Option<Uuid>,
        return_to: Option<String>,
        idle_timeout_seconds: Option<u32>,
    },
    /// A new enrol link for a device that has not bound yet, or lost its link.
    ReissueLink { id: Uuid },
    /// End a device: its tablet can no longer take a PIN.
    Revoke { id: Uuid },
}

/// One source report to put in a pipeline's landing zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceReport {
    /// The file's bytes.
    pub file: Vec<u8>,
    /// Its name; the period is read from it when none is given.
    pub file_name: String,
    /// The pipeline's name, as the manifest and the approval name it.
    pub pipeline: String,
    /// Names the object; re-sending one id for one period replaces it.
    pub workflow_id: String,
    /// `(year, month)`, or `None` to read it from the file name.
    pub period: Option<(i64, u32)>,
}

/// Where a pipeline named in an upload lives: found by
/// [`AppPlatformActions::resolve_pipeline`] among the org's compiled
/// pipelines, before access is decided on it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPipeline {
    /// The workspace whose promoted revision holds the pipeline. The upload
    /// route still needs one; nothing outside the host op names it.
    pub workspace_id: Uuid,
    /// The pipeline file's path in that revision, as the route's
    /// `pipeline_ref` takes it.
    pub file_path: String,
    /// Its compiled definition, read with the lookup.
    pub definition: Value,
}

/// Why the platform did not do what a function asked, in words the function
/// may see. The detail of an internal failure is logged, never returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformActionError {
    /// The request itself cannot be honoured: a bad name, an unknown location,
    /// a report the source would refuse. The platform's own words.
    Invalid(String),
    /// Nothing by that id, in this org.
    NotFound(String),
    /// The platform could not do it just now.
    Unavailable(String),
}

/// What a [`AppPlatformActions`] method answers: the reply body the function
/// sees, or why not. Boxed rather than `async fn` so the implementation in
/// `oxy-server` needs no macro, as [`crate::function_seam::FunctionRuntime`].
pub type PlatformFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, PlatformActionError>> + Send + 'a>>;

/// The platform's work for an app's approved actions.
pub trait AppPlatformActions: Send + Sync {
    /// A kiosk-device operation in `org_id`. `actor_user_id` is recorded as
    /// whoever made a device (the app's service account).
    fn frontline_devices<'a>(
        &'a self,
        db: &'a DatabaseConnection,
        org_id: Uuid,
        op: FrontlineDevicesOp,
        actor_user_id: Uuid,
    ) -> PlatformFuture<'a>;

    /// Find the pipeline named `name` in `org_id`: the one compiled pipeline of
    /// that name across the promoted revisions of the org's workspaces.
    /// `NotFound` when there is none, `Invalid` when there is more than one.
    fn resolve_pipeline<'a>(
        &'a self,
        db: &'a DatabaseConnection,
        org_id: Uuid,
        name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedPipeline, PlatformActionError>> + Send + 'a>>;

    /// Put a source report in `pipeline`'s landing zone, checked by the
    /// pipeline's own source first.
    fn source_upload_report<'a>(
        &'a self,
        pipeline: ResolvedPipeline,
        report: SourceReport,
    ) -> PlatformFuture<'a>;
}

static INSTALLED: OnceLock<Arc<dyn AppPlatformActions>> = OnceLock::new();

/// Install this process's implementation. First install wins: the composition
/// root hands in one, so a second is the same one.
pub fn install(actions: Arc<dyn AppPlatformActions>) {
    let _ = INSTALLED.set(actions);
}

/// The installed implementation, if any.
pub fn installed() -> Option<Arc<dyn AppPlatformActions>> {
    INSTALLED.get().cloned()
}
