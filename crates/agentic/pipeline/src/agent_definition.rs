//! Reading an analytics agent's definition, with the distinctions its callers
//! need: whether there was a definition at all, and whether "could not look"
//! is worth a retry.
//!
//! A run started over HTTP answers a missing agent as "not found" (`404`) and
//! a read that failed as [`PipelineError::Unavailable`] (`503`). The Workspace
//! Health probe cannot use either: an agent it could not find or read is a
//! probe it could not run (Degraded), while an agent it found and could not
//! parse is a broken agent (Unhealthy).
//!
//! The working-copy fallback reads `base_dir/<agent_id>`, and `agent_id`
//! arrives from HTTP bodies and queued runs. `Path::join` lets an absolute path
//! or a `..` walk out of `base_dir`, so the id must name something under the
//! root, and the file it resolves to must still be under the root once
//! symlinks are followed — or the read is
//! [`OutsideWorkspace`](AgentDefinitionError::OutsideWorkspace).

use std::path::{Path, PathBuf};

use agentic_analytics::config::{AgentConfig, ConfigError};
use agentic_automation::WorkspaceReadError;

use crate::PipelineError;
use crate::platform::PlatformContext;

/// Why an analytics agent's definition could not be had.
#[derive(Debug)]
pub(crate) enum AgentDefinitionError {
    /// There was nothing to read: the host looked and has no such agent, or
    /// (for a host with no compile boundary) this node's disk has no file.
    Missing(String),
    /// A definition was found and does not parse.
    Invalid(String),
    /// The host could not look: the compile boundary did not answer, or there
    /// is nothing on this node to look in yet. Not a miss, and worth a retry.
    Unavailable(String),
    /// The id is not a relative path inside the workspace, or names a file
    /// that resolves outside it. Nothing was read.
    OutsideWorkspace(String),
}

impl std::fmt::Display for AgentDefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(m) | Self::Invalid(m) | Self::Unavailable(m) => f.write_str(m),
            Self::OutsideWorkspace(agent_id) => write!(
                f,
                "agent {agent_id:?} must be a relative path inside the workspace"
            ),
        }
    }
}

impl From<WorkspaceReadError> for AgentDefinitionError {
    fn from(error: WorkspaceReadError) -> Self {
        match error {
            WorkspaceReadError::Missing(m) => Self::Missing(m),
            WorkspaceReadError::Invalid(m) => Self::Invalid(m),
            WorkspaceReadError::Unavailable(m) => Self::Unavailable(m),
        }
    }
}

impl From<AgentDefinitionError> for PipelineError {
    /// Only a read that could not happen is retryable. A missing agent keeps
    /// its "not found" text, which is what the HTTP callers answer `404` on.
    fn from(error: AgentDefinitionError) -> Self {
        match error {
            AgentDefinitionError::Unavailable(m) => {
                PipelineError::Unavailable(format!("agent definition: {m}"))
            }
            other => PipelineError::Config(other.to_string()),
        }
    }
}

/// The agent's parsed definition.
///
/// A ref that is not a relative path inside the workspace is refused before
/// either read, so the answer does not depend on what happens to be compiled.
/// A host with a compile boundary then answers for good (`resolve_agent`):
/// the row the ref names, a miss, or a failed read, and none of those reads
/// this node's disk. Only a host that does not answer (`Ok(None)`: the CLI,
/// tests) falls through to the working copy, trying `agent_id` as written and
/// then with `.agentic.yml` appended.
pub(crate) async fn read_analytics_agent(
    platform: &dyn PlatformContext,
    agent_id: &str,
    base_dir: &Path,
) -> Result<AgentConfig, AgentDefinitionError> {
    if !is_inside_workspace(agent_id) {
        return Err(AgentDefinitionError::OutsideWorkspace(agent_id.to_string()));
    }
    if let Some(yaml) = platform.resolve_agent(agent_id).await? {
        return AgentConfig::from_yaml(&yaml)
            .map_err(|e| AgentDefinitionError::Invalid(e.to_string()));
    }
    let config_path = candidate_path(agent_id, base_dir);
    // What `is_inside_workspace` cannot see: a symlink under the root that
    // points out of it. A path that does not exist falls through to the read,
    // whose "not found" a caller maps to `404`.
    if config_path.exists() && !resolves_under(&config_path, base_dir) {
        return Err(AgentDefinitionError::OutsideWorkspace(agent_id.to_string()));
    }
    read_agent_file(&config_path)
}

/// Whether a ref names something under the workspace root: non-empty, and
/// made only of plain names and `.` — no root, no prefix, no `..`.
fn is_inside_workspace(agent_ref: &str) -> bool {
    use std::path::Component;
    !agent_ref.is_empty()
        && Path::new(agent_ref)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// `base_dir/<agent_id>` when it exists, else `<agent_id>.agentic.yml` when
/// that does, else the bare join (whose read fails as "not found").
fn candidate_path(agent_id: &str, base_dir: &Path) -> PathBuf {
    let config_path = base_dir.join(agent_id);
    if config_path.exists() {
        return config_path;
    }
    let with_ext = base_dir.join(format!("{agent_id}.agentic.yml"));
    if with_ext.exists() {
        with_ext
    } else {
        config_path
    }
}

/// Whether `path`, symlinks followed, is under `base_dir`, symlinks followed.
/// Either failing to canonicalize counts as outside: nothing is read on a
/// path whose real location cannot be stated.
fn resolves_under(path: &Path, base_dir: &Path) -> bool {
    match (path.canonicalize(), base_dir.canonicalize()) {
        (Ok(real), Ok(root)) => real.starts_with(root),
        _ => false,
    }
}

/// The definition in one file: a file that cannot be read is
/// [`Missing`](AgentDefinitionError::Missing), one that does not parse
/// [`Invalid`](AgentDefinitionError::Invalid).
pub(crate) fn read_agent_file(path: &Path) -> Result<AgentConfig, AgentDefinitionError> {
    AgentConfig::from_file(path).map_err(|e| match e {
        ConfigError::Io(_) => AgentDefinitionError::Missing(e.to_string()),
        e => AgentDefinitionError::Invalid(e.to_string()),
    })
}

#[cfg(test)]
#[path = "agent_definition_tests.rs"]
mod tests;
