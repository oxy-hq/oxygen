//! The databases an analytics agent may reach, and their connectors.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agentic_analytics::config::AgentConfig;
use agentic_connector::DatabaseConnector;

use super::{PlatformContext, resolve_connectors};

/// The agent's effective databases, in order, and a connector for each that
/// resolved.
///
/// Effective set = `agent.databases` ∪ databases referenced from the agent's
/// `context:` glob. No fallback to "everything in config.yml": an agent only
/// gets connector access to what it explicitly declares. The previous
/// auto-fill (f7c474080) was unwound because a misparsed agent (e.g. classic
/// `.agent.yml` shape silently dropped to defaults) would otherwise inherit
/// the whole workspace's database surface and start chasing SQL it has no
/// business running.
pub(crate) async fn agent_connectors(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    base_dir: &Path,
) -> (Vec<String>, HashMap<String, Arc<dyn DatabaseConnector>>) {
    let mut databases: Vec<String> = config.databases.clone();
    if let Ok(resolved) = config.resolve_context(base_dir) {
        for name in resolved.referenced_databases {
            if !databases.contains(&name) {
                databases.push(name);
            }
        }
    }
    let resolved = resolve_connectors(&databases, platform).await;
    let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
    connectors.extend(resolved.pre_built);
    (databases, connectors)
}
