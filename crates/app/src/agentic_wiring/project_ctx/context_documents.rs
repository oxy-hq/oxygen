//! An agent's markdown context documents, for [`OxyProjectContext`].
//!
//! The host half of `ProjectContext::resolve_context_documents`.
//! `ConfigManager` owns the choice of source (the pinned revision, or the
//! working copy on a node reading one) and says which of three things
//! happened. This decides what a RUN does with each.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use agentic_automation::{WorkspaceContext, WorkspaceReadError};
use oxy::config::{ArtifactError, ContextDocuments};
use uuid::Uuid;

use super::OxyProjectContext;
use crate::server::api::custom_apps_staging_pin::current_staging_pin;

impl OxyProjectContext {
    /// Always `Ok(Some(_))` unless the read failed: this host answers on every
    /// role, so the solver never falls back to globbing the context root for
    /// markdown. That fallback is what read nothing on a pod with no working
    /// copy.
    pub(super) async fn context_documents(
        &self,
        patterns: &[String],
    ) -> Result<Option<Vec<String>>, WorkspaceReadError> {
        match self
            .workspace_manager
            .config_manager
            .context_documents(patterns)
            .await
        {
            Ok(ContextDocuments::Read(documents)) => Ok(Some(
                documents
                    .into_iter()
                    .map(|document| document.content)
                    .collect(),
            )),
            Ok(ContextDocuments::NotCompiled) => {
                self.run_without_documents().await;
                Ok(Some(Vec::new()))
            }
            Err(error) => Err(self.documents_unreadable(error)),
        }
    }

    /// The pinned revision was compiled before documents were a compiled kind
    /// and this pod has no files: the run goes ahead with none.
    ///
    /// That is exactly what such a pod did before documents were compiled, and
    /// it is why this is not a failure. Every revision in production is in
    /// this state on the day the compiled kind ships, and most agents'
    /// patterns (`./semantics/**/*`) could name a `.md` whether or not the
    /// workspace has one. Refusing would fail the first run of each of them,
    /// for a workspace that lost nothing.
    ///
    /// What changes is that it is no longer silent, and that it repairs
    /// itself: the deduped self-heal compile is asked for here, so the next
    /// run reads a revision that carries the documents. Not for a pinned
    /// staging revision (a workspace preview, a custom app's staging build):
    /// compiling `main` would not change what that request reads.
    async fn run_without_documents(&self) {
        let workspace_id = self.workspace_manager.workspace_id;
        let revision_id = self.workspace_manager.config_manager.revision_id();
        let pinned = self.holds_writes() || current_staging_pin().is_some();
        let compile_requested = !pinned && self.request_compile().await;
        if first_report(workspace_id, revision_id) {
            tracing::warn!(
                %workspace_id,
                ?revision_id,
                pinned,
                compile_requested,
                "context documents: this revision was compiled before documents were \
                 compiled and this pod holds no working copy, so the run proceeds without \
                 the agent's markdown documents, as it did before. A recompile supplies them."
            );
        }
    }

    /// Labelled by the shape of the failure, with the same line
    /// `resolve_automation_yaml` draws: `retryable()` separates "could not
    /// look" from "looked, and the content is bad". Reached only for a source
    /// that should have answered: a revision that carries documents, or a
    /// working copy.
    fn documents_unreadable(&self, error: ArtifactError) -> WorkspaceReadError {
        let retryable = error.retryable();
        tracing::warn!(
            workspace_id = %self.workspace_manager.workspace_id,
            error = ?error,
            retryable,
            "context documents could not be read"
        );
        if retryable {
            WorkspaceReadError::Unavailable(error.to_string())
        } else {
            WorkspaceReadError::Invalid(error.to_string())
        }
    }
}

/// Reports of one `(workspace, revision)` kept so the WARN above is said once
/// per process rather than once per run. Past this many it starts over, which
/// costs a repeated line and keeps the set from growing without bound.
const REPORTED_CAP: usize = 4096;

/// `true` the first time this process sees the pair.
fn first_report(workspace_id: Uuid, revision_id: Option<Uuid>) -> bool {
    static REPORTED: OnceLock<Mutex<HashSet<(Uuid, Option<Uuid>)>>> = OnceLock::new();
    let Ok(mut reported) = REPORTED.get_or_init(Default::default).lock() else {
        return true;
    };
    if reported.len() >= REPORTED_CAP {
        reported.clear();
    }
    reported.insert((workspace_id, revision_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One WARN per revision per process, not one per run: a workspace that
    /// stays on an older revision would otherwise say it on every question.
    #[test]
    fn an_older_revision_is_reported_once_and_each_revision_separately() {
        let workspace = Uuid::new_v4();
        let older = Some(Uuid::new_v4());

        assert!(first_report(workspace, older));
        assert!(!first_report(workspace, older));
        assert!(
            first_report(workspace, Some(Uuid::new_v4())),
            "a different revision of the same workspace is its own report"
        );
        assert!(first_report(Uuid::new_v4(), older));
    }
}
