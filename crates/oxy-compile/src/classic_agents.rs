//! Classic `.agent.yml` agents are retired; the compile says so.
//!
//! The classic runtime is gone: nothing loads a `*.agent.yml`, and an
//! automation step whose `agent_ref` names one fails when it runs. The one
//! agent kind is the agentic agent (`*.agentic.yml`). This module is phase 1 of
//! `internal-docs/2026-11-09-classic-agent-retirement.md`: it makes both
//! situations visible at compile time, and refuses neither. A later phase turns
//! the warnings into a refusal behind a flag, once the census says who is left.
//!
//! Two warnings, one channel. They are `tracing::warn!` events with a stable
//! `event` field ([`EVENT`]), the channel the retired-automation-step warning
//! already uses, so platform telemetry can count the workspaces that still hold
//! a classic file. There is no per-revision diagnostics column to add them to:
//! `revisions.error_summary` carries failures only, and a warning is not one.
//!
//! - a discovered `*.agent.yml` file ([`FileKind::RetiredAgent`]), which
//!   compiles to no row;
//! - an automation step whose `agent_ref` names a `.agent.yml` path, or names
//!   no agent that compiled in this revision.
//!
//! The ref check runs over the compiled rows, after every file has compiled, so
//! "an agent behind it" means exactly the `agent_definitions` rows the revision
//! will carry.
//!
//! [`FileKind::RetiredAgent`]: crate::walker::FileKind::RetiredAgent

use crate::automation_check::visit_tasks;
use crate::compile::{CompiledAutomation, CompiledRow};
use crate::walker::DiscoveredFile;
use serde_json::Value;
use std::collections::HashSet;
use tracing::warn;

/// The `event` field every warning here carries, for telemetry queries.
pub(crate) const EVENT: &str = "classic_agent_retired";

/// What the author should do about a classic file.
const MIGRATE: &str = "Convert it to a `.agentic.yml` agent; the planned `oxy migrate-agents` \
     command will translate classic files (it does not exist yet)";

/// One warning, located at the file that holds the problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassicAgentWarning {
    pub path: String,
    pub message: String,
}

impl ClassicAgentWarning {
    /// Log it. A `warn!` per file: a workspace holds a handful of agents, and
    /// the compile is the only place the author is told.
    pub(crate) fn log(&self) {
        warn!(path = %self.path, event = EVENT, "{}", self.message);
    }
}

/// The warning for a discovered classic file, naming the fields it uses so the
/// census does not need a second pass over the repository.
pub(crate) async fn retired_file_warning(file: &DiscoveredFile) -> ClassicAgentWarning {
    let fields = match tokio::fs::read_to_string(&file.abs_path).await {
        Ok(content) => classic_fields(&content),
        Err(_) => String::new(),
    };
    ClassicAgentWarning {
        path: file.rel_path.clone(),
        message: format!(
            "classic agent format (`.agent.yml`) is retired: nothing runs this file and it \
             compiles to no agent{fields}. {MIGRATE}"
        ),
    }
}

/// ` (fields: model, tools[execute_sql, retrieval])`, or empty when the file
/// does not parse as a mapping. Best effort: the file is not being compiled.
fn classic_fields(content: &str) -> String {
    let Ok(Value::Object(map)) = serde_yaml::from_str::<Value>(content) else {
        return String::new();
    };
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    let tools = map.get("tools").and_then(Value::as_array);
    let kinds: Vec<&str> = tools
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("type").and_then(Value::as_str))
        .collect();
    let rendered: Vec<String> = keys
        .iter()
        .map(|k| match (k.as_str(), kinds.is_empty()) {
            ("tools", false) => format!("tools[{}]", kinds.join(", ")),
            _ => k.clone(),
        })
        .collect();
    format!(" (fields: {})", rendered.join(", "))
}

/// Warnings for every automation step whose `agent_ref` names a classic file or
/// no compiled agent. `rows` is the whole revision's compiled rows.
pub(crate) fn agent_ref_warnings(rows: &[CompiledRow]) -> Vec<ClassicAgentWarning> {
    let known = known_agent_refs(rows);
    let mut out = Vec::new();
    for row in rows {
        if let CompiledRow::Automation(automation) = row {
            out.extend(warnings_for(automation, &known));
        }
    }
    out
}

/// THE normalisation of an `agent_ref`: the spellings it may resolve by, most
/// specific first. The full ref (a workspace path or a bare name), the ref minus
/// `.agentic.yml`/`.agentic.yaml`, and the last `/` segment of that (the stem
/// the runtime looks up `agent_definitions.name` by today,
/// `OxyProjectContext::resolve_agent_yaml`). One home for the rule so the
/// runtime can adopt it instead of repeating it.
pub fn agent_ref_candidates(target: &str) -> Vec<String> {
    let full = target.trim().trim_start_matches("./");
    let bare = strip_agent_extension(full);
    let stem = bare.rsplit('/').next().unwrap_or(bare);
    let mut out = vec![full.to_string()];
    for c in [bare, stem] {
        if !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    }
    out
}

fn strip_agent_extension(path: &str) -> &str {
    path.strip_suffix(".agentic.yml")
        .or_else(|| path.strip_suffix(".agentic.yaml"))
        .unwrap_or(path)
}

/// Every spelling a compiled agent answers to: its path, its path minus the
/// extension, its last-segment stem, and its compile key (`name:`). Deliberately
/// a superset of what the runtime resolves (stem to name today, path first in
/// phase 0a): while this only warns, a missed warning costs less than a false
/// "cannot run".
fn known_agent_refs(rows: &[CompiledRow]) -> HashSet<String> {
    let mut known = HashSet::new();
    for row in rows {
        if let CompiledRow::Agent(agent) = row {
            known.insert(agent.name.clone());
            known.extend(agent_ref_candidates(&agent.file_path));
        }
    }
    known
}

fn warnings_for(
    automation: &CompiledAutomation,
    known: &HashSet<String>,
) -> Vec<ClassicAgentWarning> {
    let mut out = Vec::new();
    let Some(tasks) = automation.definition.get("tasks") else {
        return out;
    };
    visit_tasks(tasks, "tasks", &mut |location, step| {
        let Some(agent_ref) = step.get("agent_ref").and_then(Value::as_str) else {
            return;
        };
        let target = agent_ref.trim().trim_start_matches("./");
        // `__builder__` is a retired step of its own (`automation_check`).
        if target.is_empty() || target == "__builder__" {
            return;
        }
        let problem = if is_classic(target) {
            "names a classic `.agent.yml` agent, which is retired and cannot run"
        } else if !agent_ref_candidates(target)
            .iter()
            .any(|c| known.contains(c))
        {
            "names no agent that compiled in this workspace, so the step cannot run"
        } else {
            return;
        };
        out.push(ClassicAgentWarning {
            path: automation.file_path.clone(),
            message: format!(
                "step {location}: `agent_ref: {target}` {problem}. Point it at a `.agentic.yml` \
                 agent; for a classic file, {}",
                MIGRATE.replacen("Convert", "convert", 1)
            ),
        });
    });
    out
}

fn is_classic(target: &str) -> bool {
    target.ends_with(".agent.yml") || target.ends_with(".agent.yaml")
}

#[cfg(test)]
#[path = "classic_agents_tests.rs"]
mod tests;
