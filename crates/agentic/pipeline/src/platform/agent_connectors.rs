//! The databases an analytics agent may reach, and their connectors.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use agentic_analytics::config::AgentConfig;
use agentic_connector::DatabaseConnector;

use super::{PlatformContext, resolve_connectors};

/// The agent's effective databases, in order.
///
/// Effective set = `agent.databases` ∪ databases referenced from the agent's
/// `context:` glob. No fallback to "everything in config.yml": an agent only
/// gets connector access to what it explicitly declares. The previous
/// auto-fill (f7c474080) was unwound because a misparsed agent (e.g. classic
/// `.agent.yml` shape silently dropped to defaults) would otherwise inherit
/// the whole workspace's database surface and start chasing SQL it has no
/// business running.
fn agent_databases(config: &AgentConfig, base_dir: &Path) -> Vec<String> {
    let mut databases: Vec<String> = config.databases.clone();
    if let Ok(resolved) = config.resolve_context(base_dir) {
        for name in resolved.referenced_databases {
            if !databases.contains(&name) {
                databases.push(name);
            }
        }
    }
    databases
}

/// The agent's effective databases ([`agent_databases`]) and a connector for
/// each that resolved, as the FSM queries them: over the requester's role.
pub(crate) async fn fsm_connectors(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    base_dir: &Path,
) -> (Vec<String>, Connectors) {
    let databases = agent_databases(config, base_dir);
    let connectors = role_connectors(platform, &databases).await;
    (databases, connectors)
}

/// The agent's effective databases ([`agent_databases`]) and a connector for
/// each that resolved, as a react run queries them: the platform's read-only
/// one where it has one ([`PlatformContext::read_only_connector`]), else the
/// FSM's.
///
/// The read-only connector is asked for first so a database that has one
/// never builds the role connection too. Built and then swapped, as before,
/// an `airhouse_managed` database minted a credential for the requester's
/// role (an Owner's is Admin) and opened a pooled connection on it that the
/// run never queried.
pub(crate) async fn react_connectors(
    platform: &dyn PlatformContext,
    config: &AgentConfig,
    base_dir: &Path,
) -> (Vec<String>, Connectors) {
    let databases = agent_databases(config, base_dir);
    let mut connectors = HashMap::new();
    let mut rest = Vec::new();
    for name in &databases {
        match platform.read_only_connector(name).await {
            Some(read_only) => {
                connectors.insert(name.clone(), read_only);
            }
            None => rest.push(name.clone()),
        }
    }
    connectors.extend(role_connectors(platform, &rest).await);
    (databases, connectors)
}

type Connectors = HashMap<String, Arc<dyn DatabaseConnector>>;

/// A connector for each of `databases` that resolved, over the requester's
/// role.
async fn role_connectors(platform: &dyn PlatformContext, databases: &[String]) -> Connectors {
    let resolved = resolve_connectors(databases, platform).await;
    let mut connectors = agentic_connector::build_named_connectors(resolved.configs).await;
    connectors.extend(resolved.pre_built);
    connectors
}

#[cfg(test)]
#[path = "agent_connectors_tests.rs"]
mod tests;
