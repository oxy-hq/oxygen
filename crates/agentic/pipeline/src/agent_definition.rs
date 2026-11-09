//! Reading an analytics agent's definition, with the one distinction its
//! callers need: whether there was a definition at all.
//!
//! A run started over HTTP flattens both outcomes into
//! [`PipelineError::Config`](crate::PipelineError::Config). The Workspace
//! Health probe cannot: an agent it could not find is a probe it could not run
//! (Degraded), while an agent it found and could not parse is a broken agent
//! (Unhealthy).

use std::path::Path;

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
}

impl std::fmt::Display for AgentDefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(e) | Self::Invalid(e) => write!(f, "{e}"),
        }
    }
}

/// The agent's parsed definition.
///
/// `resolve_agent_yaml` returns the compiled definition when the compile
/// boundary has one; `None` falls through to this node's working copy, trying
/// `agent_id` as written and then with `.agentic.yml` appended.
pub(crate) async fn read_analytics_agent(
    platform: &dyn PlatformContext,
    agent_id: &str,
    base_dir: &Path,
) -> Result<AgentConfig, AgentDefinitionError> {
    if let Some(yaml) = platform.resolve_agent_yaml(agent_id).await {
        return AgentConfig::from_yaml(&yaml).map_err(AgentDefinitionError::Invalid);
    }
    let config_path = base_dir.join(agent_id);
    let config_path = if config_path.exists() {
        config_path
    } else {
        let with_ext = base_dir.join(format!("{agent_id}.agentic.yml"));
        if with_ext.exists() {
            with_ext
        } else {
            config_path // will produce a clear "not found" error
        }
    };
    read_agent_file(&config_path)
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
