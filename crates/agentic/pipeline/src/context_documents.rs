//! An analytics agent's markdown context documents, asked of the host before
//! the solver is built.
//!
//! The solver used to find them itself, by globbing the agent's `context:`
//! under the context root. That root is the working copy on a node that has
//! one and a materialised copy of the compiled revision elsewhere, and
//! markdown was never in the second: a run on a pod with no working copy
//! started without its documents and reported nothing.
//!
//! The host now answers ([`ProjectContext::resolve_context_documents`]), from
//! the revision the agent's own definition came from, and the answer replaces
//! the glob wherever there is one.

use agentic_analytics::config::AgentConfig;
use agentic_automation::WorkspaceReadError;

use crate::PipelineError;
use crate::platform::ProjectContext;

/// The documents `config`'s `context:` reaches, or `None` when the host does
/// not answer and the solver should read them from the context root.
///
/// An error stops the run from being built. Starting without documents the
/// agent may have is the outcome this exists to rule out, so "could not find
/// out" is never downgraded to "has none".
pub(crate) async fn resolve<P: ProjectContext + ?Sized>(
    platform: &P,
    config: &AgentConfig,
) -> Result<Option<Vec<String>>, PipelineError> {
    platform
        .resolve_context_documents(&config.context)
        .await
        .map_err(start_error)
}

/// Labelled by what the host said, not by where the call sits: only
/// `Unavailable` is worth a retry.
fn start_error(error: WorkspaceReadError) -> PipelineError {
    if error.is_unavailable() {
        PipelineError::Unavailable(format!("context documents: {error}"))
    } else {
        PipelineError::Config(format!("context documents: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Not compiled yet" has to reach the caller as something it can retry,
    /// and a broken file as something it cannot.
    #[test]
    fn only_an_unavailable_host_is_a_retryable_start_failure() {
        let unavailable = start_error(WorkspaceReadError::Unavailable("not compiled yet".into()));
        assert!(unavailable.is_retryable(), "{unavailable}");
        assert!(unavailable.to_string().contains("not compiled yet"));

        for permanent in [
            WorkspaceReadError::Invalid("bad file".into()),
            WorkspaceReadError::Missing("no such file".into()),
        ] {
            let error = start_error(permanent);
            assert!(!error.is_retryable(), "{error}");
        }
    }
}
