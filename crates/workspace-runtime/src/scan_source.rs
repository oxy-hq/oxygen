//! Where a semantic read finds the model: the compiled revision first, the
//! working copy second where this node holds one, and the refusal (with a
//! deduped compile enqueued) when neither is there.
//!
//! The workspace runtime's half of the semantic surface. The project context
//! (`project_ctx`), the metric-tree executor (`metric_tree_exec`), the
//! pre-aggregation executor and the global driver's workspace-health context
//! resolve their scan here, and so do the IDE's semantic and pre-aggregation
//! handlers and `oxy-api-world-model` / `oxy-api-experiment` (through the
//! prelude's `workspace_runtime` until decompose R1, directly since). It lived in `api::semantic` until
//! 2026-11-16 and moved down so those handlers can leave `oxy-app` without
//! taking the runtime's scan resolution with them; it was `oxy-app`'s
//! `agentic_wiring::scan_source` until 2026-11-22 (re-exported there). With
//! it, the `{ message }` envelope the semantic routes answer a refusal in.
//!
//! Not a domain crate: both resolvers enqueue a compile on a miss
//! (`oxy_compile_runtime::reconcile::enqueue::enqueue_lazy_compile`, which
//! reaches the compile self-heal and the reconcile check), so it sits in the
//! workspace runtime, above the compile runtime.

use axum::{extract, http::StatusCode};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::scan::{self, ScanDir};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub message: String,
}

pub fn semantic_err(
    status: StatusCode,
    message: String,
) -> (StatusCode, extract::Json<ErrorResponse>) {
    (status, extract::Json(ErrorResponse { message }))
}

/// Where a semantic **query** (compile / execute) reads the layer from.
///
/// The query-shaped counterpart to `oxy-api-semantic`'s `semantic::resolve_semantic_source`,
/// which does the same for a single-file read. On a serve replica the scan is the compiled
/// revision's files, held in memory, so there is nothing to keep alive; where a
/// working copy is held it is that directory.
pub struct QueryScanSource {
    pub scan: ScanDir,
}

impl QueryScanSource {
    /// A scan rooted at a directory that is already on disk — the working-copy
    /// arm. Lets a caller that resolved a root some other way (or a test) hand
    /// one to something that takes a `QueryScanSource`.
    pub fn at(scan_path: std::path::PathBuf) -> Self {
        Self {
            scan: ScanDir::Real(scan_path),
        }
    }

    /// The revision this scan actually READ, or `None` for the working copy.
    pub fn source_revision<S: oxy::config::DiskSlot>(
        &self,
        workspace_manager: &WorkspaceManager<S>,
    ) -> Option<Uuid> {
        scan_source_revision(&Some(self.scan.clone()), workspace_manager)
    }
}

/// The revision a resolved scan actually READ, or `None` for the working copy.
///
/// Not `config_manager.revision_id()`: that reports the revision the request is
/// pinned to, which is `Some` even on a node serving its own working copy.
/// Caching by the pin instead of the source lets a working-copy reader and a
/// revision reader share one layer, and one engine built from it.
///
/// A free function, not a method, because both resolvers have to answer it and
/// they return different shapes: `resolve_query_scan_source` wraps its guard in
/// [`QueryScanSource`], `oxy-api-semantic`'s `semantic::resolve_semantic_source` hands the
/// `Option<ScanDir>` back bare. One rule, one place.
pub fn scan_source_revision<S: oxy::config::DiskSlot>(
    guard: &Option<ScanDir>,
    workspace_manager: &WorkspaceManager<S>,
) -> Option<Uuid> {
    match guard {
        Some(scan) if scan.is_compiled() => workspace_manager.config_manager.revision_id(),
        _ => None,
    }
}

/// The workspace has no compiled semantic model and this node has no working
/// copy to fall back to.
pub struct ScanUnavailable {
    workspace_id: Uuid,
    /// The caller refused the working-copy fallback rather than lacking one.
    /// Same absence, different thing to tell the reader: one is "this node
    /// doesn't hold the files", the other is "these files are not the ones I am
    /// allowed to judge".
    compiled_only: bool,
}

impl ScanUnavailable {
    pub fn message(&self) -> String {
        if self.compiled_only {
            return format!(
                "workspace {} has no compiled semantic model; this check reads the promoted \
                 revision only and will not fall back to a working copy — a (re)compile has \
                 been enqueued",
                self.workspace_id
            );
        }
        format!(
            "workspace {} has no compiled semantic model available on this stateless \
             replica; a (re)compile has been enqueued — retry shortly",
            self.workspace_id
        )
    }
}

/// Resolve the scan root for a semantic query — compile boundary first, working
/// copy second.
///
/// Both of the semantic query handlers (`api::semantic`'s then, `oxy-api-semantic`'s now) used to read `semantics_scan_path()`
/// unconditionally, which is the workspace working copy. A stateless `serve`
/// replica has no working copy, and scanning a directory that isn't there
/// produced an EMPTY semantic model rather than an error — surfacing as
/// `Topic 'x' not found. Available: []`, i.e. a modelling mistake for what is
/// really a missing directory. Both routes are (correctly) classified `FleetOk`
/// in `role_manifest`, so they must serve from Postgres like their siblings:
/// `oxy-api-custom-apps`' `data_plane::semantic_query` (custom-app data plane) and
/// `oxy-api-semantic`'s `semantic::resolve_semantic_source` (single-file IDE reads) already
/// do exactly this.
///
/// The workspace-surface metric-tree handlers (`oxy-api-metric-monitoring`'s `metric_tree`, through
/// `metric_tree_exec::resolve_scan`) are the
/// other caller, for the same reason and with a louder symptom: scanning the
/// missing directory failed outright there, 500ing every metric-tree call on
/// every workspace in cloud (oxy-hq/oxygen#878).
///
/// Branch semantics come for free: `workspace_middleware` pins the request to
/// one revision via `compiled_reader::resolve_request_revision`, which yields
/// `None` for a non-default branch on a node that HAS a working copy. The IDE
/// previewing uncommitted edits on a feature branch therefore still reads the
/// FS, exactly as before.
pub async fn resolve_query_scan_source<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Result<QueryScanSource, ScanUnavailable> {
    let workspace_id = workspace_manager.workspace_id;
    // `scan_dir` covers both arms — the working copy's own path, or the compiled
    // rows written out — so it only fails when neither is available.
    match scan::scan_dir(&workspace_manager.config_manager).await {
        Ok(scan) => return Ok(QueryScanSource { scan }),
        Err(e) => tracing::warn!(
            workspace_id = %workspace_id,
            error = %e,
            "semantic query: no scan directory available"
        ),
    }

    // Stateless-fleet guard, mirroring `data_plane::semantic_query`: refuse the FS
    // fallback on a node that has no working copy and enqueue a deduped compile
    // so the next request succeeds without operator action. Note
    // `materialise_semantic_scan` downgrades real DB errors to `None`, so this
    // also covers a transient DB failure — a retry is the right answer there too.
    //
    // One question, asked once — and asked of `semantics_scan_dir`, which goes
    // through `disk()`.
    //
    // This asked `working_copy()`, i.e. "is there a handle?", and on a replica
    // the answer is always yes: `effective_workspace_path` returns the database
    // column without stat-ing it and `ReadOnly` keeps the slot. So the `Ok` arm
    // always won, handing airlayer a directory the node does not have — the
    // "Topic not found. Available: []" this guard exists to prevent — and the
    // `ScanUnavailable` arm below, the one that enqueues the compile, could
    // never run.
    let Ok(scan_path) = workspace_manager.config_manager.semantics_scan_dir() else {
        if let Ok(db) = oxy::database::client::establish_connection().await {
            oxy_compile_runtime::reconcile::enqueue::enqueue_lazy_compile(&db, workspace_id).await;
        }
        return Err(ScanUnavailable {
            workspace_id,
            compiled_only: false,
        });
    };

    Ok(QueryScanSource::at(scan_path))
}

/// [`resolve_query_scan_source`] for a reader that must JUDGE the workspace
/// rather than serve it: the promoted revision only, never the working copy.
///
/// The workspace-health eval is that reader. It can run on a node that holds a
/// working copy — a single instance, or the factory doubling as the global-run
/// node — and that copy holds whatever someone is editing in the IDE right now.
/// Falling back to it means a half-written `.view.yml` on a feature branch
/// pages the on-call about a workspace whose promoted revision, and therefore
/// everything actually being served, is fine.
///
/// The `Err` arm is a real answer, not a failure to look: the workspace has no
/// compiled semantic model, so health has no opinion about its semantic layer
/// and says so (Degraded — "we learned nothing"). A compile is enqueued, so the
/// next pass succeeds without operator action. A workspace with no promoted
/// config has no health schedule row at all, so in practice this is the manual
/// `POST /workspace-health/{id}/eval` and the mid-deploy window.
pub async fn resolve_compiled_scan_source<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Result<QueryScanSource, ScanUnavailable> {
    let workspace_id = workspace_manager.workspace_id;
    match oxy::config::scan::compiled_scan_dir(&workspace_manager.config_manager).await {
        Ok(scan) => Ok(QueryScanSource { scan }),
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %e,
                "compiled-only semantic scan: nothing compiled to read"
            );
            if let Ok(db) = oxy::database::client::establish_connection().await {
                oxy_compile_runtime::reconcile::enqueue::enqueue_lazy_compile(&db, workspace_id)
                    .await;
            }
            Err(ScanUnavailable {
                workspace_id,
                compiled_only: true,
            })
        }
    }
}

#[cfg(test)]
mod scan_source_tests {
    use super::*;

    /// A node that does not hold the files must refuse, not hand back a path.
    ///
    /// This asked `working_copy()` — "is there a handle?" — and on a replica the
    /// answer is always yes: `effective_workspace_path` returns the database
    /// column without stat-ing it, and `ReadOnly` keeps the slot. So the `Ok`
    /// arm always won and airlayer scanned a directory that is not on the node,
    /// finding nothing: the "Topic not found. Available: []" this guard exists
    /// to prevent. The `ScanUnavailable` arm — the one that enqueues a compile
    /// so the next request succeeds — was unreachable in production.
    ///
    /// Built the replica way: slot FULL, directory absent.
    #[tokio::test]
    async fn a_node_without_the_files_refuses_instead_of_scanning_thin_air() {
        let parent = tempfile::tempdir().expect("tempdir");
        let absent = parent.path().join("never-cloned");

        let manager = oxy::adapters::workspace::builder::WorkspaceBuilder::new(uuid::Uuid::nil())
            .with_working_copy(&absent, None, oxy::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .build()
            .await
            .expect("workspace manager");

        assert!(
            manager.config_manager.working_copy().is_some(),
            "the slot is full — that is the trap this guards"
        );
        assert!(!absent.is_dir(), "and the directory is not there");

        let refused = resolve_query_scan_source(&manager).await;
        assert!(
            refused.is_err(),
            "a scan path from a node that holds nothing is worse than an error: \
             airlayer reports an empty layer and the caller blames the workspace"
        );
    }

    /// Every variant of the message keeps the phrase the agentic coordinator
    /// matches on.
    ///
    /// `agentic_runtime::…::coordinator::outcomes::is_pending_compile` demotes
    /// this failure from ERROR to WARN by substring, because the formatted
    /// string is all a domain-agnostic runtime ever receives — and
    /// `agentic-runtime` may not depend on this crate, so the two cannot share
    /// a const. That leaves the coupling invisible from the other side: reword
    /// either arm without this phrase and nothing fails, while the coordinator
    /// quietly goes back to reporting a self-healing retry as the loudest
    /// ERROR in production (993 events in the week to 2026-09-21). This test is
    /// the thing that fails instead.
    #[test]
    fn both_messages_carry_the_pending_compile_marker() {
        const MARKER: &str = "a (re)compile has been enqueued";
        for compiled_only in [false, true] {
            let message = ScanUnavailable {
                workspace_id: Uuid::nil(),
                compiled_only,
            }
            .message();
            assert!(
                message.contains(MARKER),
                "the compiled_only={compiled_only} message no longer contains {MARKER:?}, so \
                 the coordinator will log it at ERROR again: {message}"
            );
        }
    }
}
