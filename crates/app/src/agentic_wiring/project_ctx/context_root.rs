//! Where an agent run resolves its `context:` globs (views, topics, SQL,
//! automations): `WorkspaceContext::context_root` for [`OxyProjectContext`].
//!
//! Chosen by the manager's `Origin`, not by the process role. A manager pinned
//! to a compiled revision materialises that revision's context rows into a
//! tempdir (`materialise_agent_context`, cached per revision) on every role,
//! the Factory included. The agent's definition already comes from that
//! revision (`ConfigManager::agent_by_ref`), so its context does too: the
//! Factory used to pair a revision's agent with the working copy's views, which
//! a fleet pod could never reproduce (gap G7 of the classic-agent retirement
//! memo).
//!
//! The Builder is the exception: it writes files, so on a node holding the
//! working copy its root is the working copy on every origin
//! (`WorkspaceContext::working_root`). The materialised dir is a cache shared by
//! every run of the revision; an edit written into it would be lost.
//!
//! The working copy is the root only when the manager reads it (`Origin::Disk`:
//! a draft branch in the IDE, the CLI), or when a compiled read had nothing to
//! give. On a stateless role that last case is logged loudly, because the
//! working copy is not there to fall back to.
//!
//! Markdown documents do not come from this root on any role: the run asks
//! `resolve_context_documents`, which reads the pinned revision — see
//! `context_documents.rs`.

use agentic_automation::{ContextRoot, WorkspaceContext};

use super::OxyProjectContext;
use crate::server::role_manifest::{Role, current_process_role};

/// What a run does with its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    /// Reads `context:` globs: an analytics run.
    Reads,
    /// Writes workspace files: the Builder.
    Writes,
}

/// Whether to try the compiled context before the working copy.
///
/// A reading run on a compiled origin does, on every role (G7). A writing run
/// never does on a node that holds the working copy: the materialised dir is a
/// per-revision cache shared by every reader, so an edit written there would
/// be lost to the user and served as the revision's context. A stateless role
/// tries for either, as it always has, so the miss is reported where it
/// happens rather than surfacing later as "no databases configured".
fn tries_compiled(run: Run, compiled_origin: bool, stateless_role: bool) -> bool {
    stateless_role || (run == Run::Reads && compiled_origin)
}

impl OxyProjectContext {
    pub(super) async fn agent_context_root(&self) -> ContextRoot {
        self.root_for(Run::Reads).await
    }

    /// The Builder's root: the working copy on the Factory whatever the
    /// origin; on a stateless role, what it always was.
    pub(super) async fn builder_root(&self) -> ContextRoot {
        self.root_for(Run::Writes).await
    }

    async fn root_for(&self, run: Run) -> ContextRoot {
        let manager = &self.workspace_manager.config_manager;
        let stateless = matches!(current_process_role(), Role::Serve | Role::Worker);
        if tries_compiled(run, manager.revision_id().is_some(), stateless) {
            match crate::server::api::semantic_scan::materialise_agent_context(manager).await {
                Ok(Some(materialised)) => {
                    // The revision travels with the root because anything a
                    // caller caches off this path must be keyed by it: a
                    // materialised root is revision-shaped, and caching an
                    // engine built from revision A under a working-copy key
                    // would serve it for revision B. `materialise_agent_context`
                    // returns `Some` only under `Origin::Compiled`, so the
                    // fallback to nil is belt-and-braces.
                    let root = materialised.root.clone();
                    let revision_id = manager.revision_id().unwrap_or_else(uuid::Uuid::nil);
                    return ContextRoot::materialised(root, Box::new(materialised), revision_id);
                }
                Ok(None) => self.log_no_compiled_context(stateless),
                Err(e) => self.log_compiled_context_failed(stateless, &e),
            }
        }
        // `Origin::Disk`, or a compiled read that had nothing to give. On a
        // stateless role the root names a directory that is not there, which
        // resolves no globs — the same outcome as the absent directory.
        ContextRoot::fs(
            self.workspace_path()
                .map(|p| p.to_path_buf())
                .unwrap_or_default(),
        )
    }

    /// The two arms below give an operator OPPOSITE instructions — recompile,
    /// or retry before recompiling — which is why they stay distinct.
    fn log_no_compiled_context(&self, stateless: bool) {
        let workspace_id = self.workspace_manager.workspace_id;
        if stateless {
            tracing::error!(
                workspace_id = %workspace_id,
                role = ?current_process_role(),
                "context_root: no compiled agent context on a stateless node — workspace not promoted / not yet compiled; the run will have no usable context. Recompile the workspace."
            );
        } else {
            tracing::debug!(
                workspace_id = %workspace_id,
                "context_root: the compiled revision holds no agent context; using the working copy"
            );
        }
    }

    fn log_compiled_context_failed(&self, stateless: bool, e: &oxy::config::ArtifactError) {
        let workspace_id = self.workspace_manager.workspace_id;
        if stateless {
            tracing::error!(
                workspace_id = %workspace_id,
                error = ?e,
                retryable = e.retryable(),
                "context_root: could not read the compile boundary on a stateless node; the run will fall through to an absent FS and fail. The workspace may be fine — retry before recompiling."
            );
        } else {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = ?e,
                retryable = e.retryable(),
                "context_root: could not read the compiled agent context; using the working copy"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Run, tries_compiled};

    /// G7: a compiled origin materialises for an analytics run on the Factory
    /// too, not only on the fleet; a draft branch (`Origin::Disk`) on the
    /// Factory reads its files.
    #[test]
    fn an_analytics_run_follows_the_origin_not_the_role() {
        assert!(
            tries_compiled(Run::Reads, true, false),
            "Factory, compiled revision"
        );
        assert!(
            tries_compiled(Run::Reads, true, true),
            "fleet, compiled revision"
        );
        assert!(
            !tries_compiled(Run::Reads, false, false),
            "Factory, draft branch"
        );
        assert!(
            tries_compiled(Run::Reads, false, true),
            "fleet, nothing compiled: logged"
        );
    }

    /// The Builder writes files: on the Factory it works in the working copy
    /// even when the manager reads a compiled revision, never in the shared
    /// materialised cache.
    #[test]
    fn a_builder_run_on_the_factory_works_in_the_working_copy() {
        assert!(
            !tries_compiled(Run::Writes, true, false),
            "Factory, compiled revision"
        );
        assert!(
            !tries_compiled(Run::Writes, false, false),
            "Factory, draft branch"
        );
        assert!(tries_compiled(Run::Writes, true, true), "fleet: unchanged");
    }
}
