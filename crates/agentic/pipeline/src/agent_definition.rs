//! Reading an analytics agent's definition, with the one distinction its
//! callers need: whether there was a definition at all.
//!
//! A run started over HTTP flattens both outcomes into
//! [`PipelineError::Config`](crate::PipelineError::Config). The Workspace
//! Health probe cannot: an agent it could not find is a probe it could not run
//! (Degraded), while an agent it found and could not parse is a broken agent
//! (Unhealthy).
//!
//! The working-copy fallback reads `base_dir/<agent_id>`, and `agent_id`
//! arrives from HTTP bodies and queued runs. `Path::join` lets an absolute path
//! or a `..` walk out of `base_dir`, so the id must name something under the
//! root, and the file it resolves to must still be under the root once
//! symlinks are followed — or the read is
//! [`OutsideWorkspace`](AgentDefinitionError::OutsideWorkspace).

use std::path::{Path, PathBuf};

use agentic_analytics::config::{AgentConfig, ConfigError};

use crate::platform::PlatformContext;

/// Why an analytics agent's definition could not be had.
#[derive(Debug)]
pub(crate) enum AgentDefinitionError {
    /// There was nothing to read: the compiled revision has no row for the
    /// agent, and this node's disk has no file (or none it could open).
    Missing(ConfigError),
    /// A definition was found and does not parse.
    Invalid(ConfigError),
    /// The id is not a relative path inside the workspace, or names a file
    /// that resolves outside it. Nothing was read.
    OutsideWorkspace(String),
}

impl std::fmt::Display for AgentDefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(e) | Self::Invalid(e) => write!(f, "{e}"),
            Self::OutsideWorkspace(agent_id) => write!(
                f,
                "agent {agent_id:?} must be a relative path inside the workspace"
            ),
        }
    }
}

/// The agent's parsed definition.
///
/// A ref that is not a relative path inside the workspace is refused before
/// either read, so the answer does not depend on what happens to be compiled.
/// `resolve_agent_yaml` returns the compiled definition when the compile
/// boundary has one; `None` falls through to this node's working copy, trying
/// `agent_id` as written and then with `.agentic.yml` appended.
pub(crate) async fn read_analytics_agent(
    platform: &dyn PlatformContext,
    agent_id: &str,
    base_dir: &Path,
) -> Result<AgentConfig, AgentDefinitionError> {
    if !is_inside_workspace(agent_id) {
        return Err(AgentDefinitionError::OutsideWorkspace(agent_id.to_string()));
    }
    if let Some(yaml) = platform.resolve_agent_yaml(agent_id).await {
        return AgentConfig::from_yaml(&yaml).map_err(AgentDefinitionError::Invalid);
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
        ConfigError::Io(_) => AgentDefinitionError::Missing(e),
        e => AgentDefinitionError::Invalid(e),
    })
}

#[cfg(test)]
#[path = "agent_definition_tests.rs"]
mod tests;
