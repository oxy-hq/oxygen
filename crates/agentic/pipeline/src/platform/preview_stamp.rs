//! A run started from a request pinned to a workspace preview is never driven
//! on the production platform.
//!
//! The platform such a run starts on reads the branch and holds every write
//! (the host's request hold), and it lives only as long as the process driving
//! the run. Anything that picks the run up later — recovery on another pod or
//! after a restart, a cold resume from a request without the preview header —
//! would drive it on the production platform: the branch's agent config would
//! be gone and nothing would hold its writes. So the run is stamped when it is
//! created ([`stamp`]), and every path that would pick it up later retires it
//! instead ([`retire_interrupted`]): the three recovery entry points (in
//! `recovery::recover_single_run`, once the driver lease is held) and
//! `PipelineBuilder::resume` on a platform that is not a preview.
//!
//! A staff dry run is not stamped: it has a [`super::PreviewScope`], and
//! recovery drives it on the preview platform its `RunPlatformResolver` picks.

use sea_orm::DatabaseConnection;

use super::PlatformContext;

/// The key a stamped run carries in `agentic_runs.metadata`:
/// `{"workspace_preview": {"revision_id": <the revision it read>}}`.
pub const RUN_STAMP: &str = "workspace_preview";

/// Why a stamped run is retired instead of driven.
pub const INTERRUPTED: &str = "preview run interrupted; start it again from the preview";

/// Whether `platform` serves a request pinned to a workspace preview — a
/// preview with no dry-run scope.
pub fn is_preview_request(platform: &dyn PlatformContext) -> bool {
    platform.is_workspace_preview() && platform.preview_scope().is_none()
}

/// Stamp a new run's `metadata` when `platform` serves a preview request. A
/// no-op otherwise, and on metadata that is not an object.
pub fn stamp(platform: &dyn PlatformContext, metadata: &mut serde_json::Value) {
    if !is_preview_request(platform) {
        return;
    }
    if let Some(map) = metadata.as_object_mut() {
        map.insert(
            RUN_STAMP.to_string(),
            serde_json::json!({ "revision_id": platform.compiled_revision() }),
        );
    }
}

/// Whether a run's `metadata` carries the stamp.
pub fn is_stamped(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|m| m.get(RUN_STAMP))
        .is_some_and(|v| !v.is_null())
}

/// Retire a stamped run as failed with [`INTERRUPTED`]: its queue rows are
/// cancelled and the run and its tree made terminal, in one transaction.
pub async fn retire_interrupted(db: &DatabaseConnection, run_id: &str) -> Result<(), String> {
    tracing::warn!(
        target: "preview",
        run_id,
        "a run started in a workspace preview was interrupted; retiring it rather than \
         driving it outside the preview"
    );
    agentic_runtime::crud::retire_run(db, run_id, INTERRUPTED)
        .await
        .map_err(|e| format!("{INTERRUPTED}; retiring it failed: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{RUN_STAMP, is_stamped, stamp};
    use crate::executor::preview_hold_tests::{PreviewAirwayPlatform, preview_scope};
    use crate::platform::PlatformContext;

    fn platform(request_preview: bool, dry_run: bool) -> Arc<dyn PlatformContext> {
        Arc::new(PreviewAirwayPlatform {
            scope: dry_run.then(preview_scope),
            request_preview,
            yaml_reads: Default::default(),
        })
    }

    fn stamped(p: &Arc<dyn PlatformContext>) -> serde_json::Value {
        let mut metadata = json!({ "agent_id": "chat" });
        stamp(p.as_ref(), &mut metadata);
        metadata
    }

    /// A chat run started from a preview request is stamped; production's and a
    /// staff dry run's are not.
    #[test]
    fn only_a_preview_request_stamps_its_runs() {
        let preview = stamped(&platform(true, false));
        assert!(is_stamped(Some(&preview)), "{preview}");
        assert!(preview[RUN_STAMP].get("revision_id").is_some(), "{preview}");
        assert_eq!(preview["agent_id"], "chat", "the rest is kept");

        let production = stamped(&platform(false, false));
        assert!(!is_stamped(Some(&production)), "{production}");
        let dry_run = stamped(&platform(false, true));
        assert!(
            !is_stamped(Some(&dry_run)),
            "a dry run is recovered on its preview platform, not retired: {dry_run}"
        );
    }

    #[test]
    fn no_metadata_or_a_null_stamp_is_unstamped() {
        assert!(!is_stamped(None));
        assert!(!is_stamped(Some(&json!({ RUN_STAMP: null }))));
        assert!(!is_stamped(Some(&json!({ "agent_id": "chat" }))));
    }

    /// `start_analytics` stamps the metadata it inserts the run with.
    #[test]
    fn a_new_analytics_run_goes_through_the_stamp() {
        let lib = include_str!("../lib.rs");
        let start = lib
            .find("async fn start_analytics(")
            .expect("start_analytics");
        let insert = lib[start..]
            .find("agentic_runtime::crud::insert_run(")
            .expect("start_analytics inserts its run");
        assert!(
            lib[start..start + insert].contains("preview_stamp::stamp("),
            "start_analytics inserts its run without `preview_stamp::stamp`"
        );
    }
}
