//! `oxy migrate-agents` — translate classic `*.agent.yml` agents into agentic
//! `*.agentic.yml` agents, and point every reference at the new files.
//!
//! Phase 1b of `internal-docs/2026-11-09-classic-agent-retirement.md`. Modelled
//! on `oxy migrate-automations`: rooted at a project directory, `--dry-run`
//! writes nothing, a file is never overwritten, and collisions are reported and
//! skipped. Unlike it, this translates content ([`translate`]), so a file with a
//! field nothing in the agentic runtime does is left in place and reported per
//! field, never half-written, unless `--force` drops those fields.
//!
//! Discovery is the compile walker's ([`oxy_compile::walker::discover`]), so the
//! skip directories and `.test.` names are exactly the ones the compile uses.
//! `--json` prints the [`Report`], which doubles as the census the memo's gate
//! reads.

mod refs;
mod report;
mod translate;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Parser;
use oxy_compile::walker::{DiscoveredFile, FileKind, discover};
use oxy_shared::errors::OxyError;

use refs::{AgentMove, FileEdit};
pub use report::Report;
use report::{Collision, Translated, Unmappable};
use translate::{TopicIndex, Translation, translate};

#[derive(Parser, Debug, Clone, Default)]
pub struct MigrateAgentsArgs {
    /// Project directory to migrate (defaults to the current directory).
    #[clap(default_value = ".")]
    pub path: String,

    /// Report what would change without writing any file.
    #[clap(long, default_value_t = false)]
    pub dry_run: bool,

    /// Print the report as JSON (the census shape) instead of text.
    #[clap(long, default_value_t = false)]
    pub json: bool,

    /// Translate files whose fields cannot be mapped too, dropping those fields.
    #[clap(long, default_value_t = false)]
    pub force: bool,
}

/// Entry point for `oxy migrate-agents`.
pub fn migrate_agents(args: MigrateAgentsArgs) -> Result<(), OxyError> {
    let report = run(&args)?;
    if args.json {
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| OxyError::SerializerError(e.to_string()))?;
        println!("{json}");
    } else {
        report.print_text();
    }
    Ok(())
}

/// Plan, and unless `--dry-run`, apply. Returns what it found and did.
pub fn run(args: &MigrateAgentsArgs) -> Result<Report, OxyError> {
    let root = PathBuf::from(&args.path);
    if !root.is_dir() {
        return Err(OxyError::ArgumentError(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let files = discover(&root).map_err(|e| OxyError::IOError(e.to_string()))?;
    let mut plan = plan_agents(&root, &files, args.force)?;
    plan.report.dry_run = args.dry_run;
    let edits = plan_refs(&root, &mut plan);
    if !args.dry_run {
        apply(&root, &plan.writes, &edits)?;
    }
    Ok(plan.report)
}

#[derive(Default)]
struct Plan {
    report: Report,
    /// Translated files and their new text.
    writes: Vec<(AgentMove, String)>,
    /// Classic files left in place (refused or collided).
    stuck: Vec<String>,
}

/// A classic file that translated, before the collision check.
struct Candidate<'a> {
    file: &'a DiscoveredFile,
    target: String,
    id: String,
    translation: Translation,
}

fn plan_agents(root: &Path, files: &[DiscoveredFile], force: bool) -> Result<Plan, OxyError> {
    let topics = topic_index(files);
    let mut plan = Plan::default();
    let mut candidates = Vec::new();
    for file in files.iter().filter(|f| f.kind == FileKind::RetiredAgent) {
        plan.report.classic_files.push(file.rel_path.clone());
        let content = std::fs::read_to_string(&file.abs_path)
            .map_err(|e| OxyError::IOError(format!("read {}: {e}", file.rel_path)))?;
        let translation = translate(&file.rel_path, &content, &topics);
        if translation.yaml.is_empty() || (!translation.refused.is_empty() && !force) {
            plan.stuck.push(file.rel_path.clone());
            plan.report.unmappable.push(Unmappable {
                path: file.rel_path.clone(),
                fields: translation.refused,
                notes: translation.notes,
            });
            continue;
        }
        let target = agentic_path(&file.rel_path);
        let id = agent_id(&target).to_string();
        candidates.push(Candidate {
            file,
            target,
            id,
            translation,
        });
    }
    let taken = existing_agent_ids(files);
    let mut per_id: HashMap<String, usize> = HashMap::new();
    for c in &candidates {
        *per_id.entry(c.id.clone()).or_default() += 1;
    }
    for c in candidates {
        match collision(root, &c, &taken, &per_id) {
            Some(reason) => {
                plan.stuck.push(c.file.rel_path.clone());
                plan.report.collisions.push(Collision {
                    path: c.file.rel_path.clone(),
                    target: c.target,
                    reason,
                });
            }
            None => accept(&mut plan, c),
        }
    }
    Ok(plan)
}

fn collision(
    root: &Path,
    c: &Candidate,
    taken: &HashMap<String, String>,
    per_id: &HashMap<String, usize>,
) -> Option<String> {
    if root.join(&c.target).exists() {
        return Some("the target already exists".into());
    }
    if let Some(other) = taken.get(&c.id) {
        return Some(format!("agent id `{}` is already {other}'s", c.id));
    }
    (per_id.get(&c.id).copied().unwrap_or(0) > 1)
        .then(|| format!("another classic file would also become agent `{}`", c.id))
}

fn accept(plan: &mut Plan, c: Candidate) {
    let mut t = c.translation;
    if let Some(name) = t.name.as_deref().filter(|n| *n != c.id) {
        t.notes.push(format!(
            "`name: {name}` was the agent's id; it is now `{}` (the file stem), and refs by \
             the old name are rewritten",
            c.id
        ));
    }
    let agent_move = AgentMove {
        classic: c.file.rel_path.clone(),
        agentic: c.target.clone(),
        name: t.name.clone(),
    };
    plan.report.translated.push(Translated {
        from: c.file.rel_path.clone(),
        to: c.target,
        dropped: t.dropped,
        forced: t.refused,
        notes: t.notes,
    });
    plan.writes.push((agent_move, t.yaml));
}

/// Rewrite references in every YAML file except the classic files themselves.
fn plan_refs(root: &Path, plan: &mut Plan) -> Vec<FileEdit> {
    let classic: HashSet<&String> = plan.report.classic_files.iter().collect();
    let moves: Vec<AgentMove> = plan.writes.iter().map(|(m, _)| m.clone()).collect();
    let mut edits = Vec::new();
    for rel in refs::yaml_files(root) {
        if classic.contains(&rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let found = refs::rewrite_file(&rel, &content, &moves, &plan.stuck);
        plan.report.refs_rewritten.extend(found.rewritten);
        plan.report.refs_blocked.extend(found.blocked);
        if let Some(content) = found.content {
            edits.push(FileEdit {
                rel_path: rel,
                content,
            });
        }
    }
    edits
}

/// Write every agentic file (never over an existing one), then delete the
/// classic files, then rewrite references.
fn apply(root: &Path, writes: &[(AgentMove, String)], edits: &[FileEdit]) -> Result<(), OxyError> {
    use std::io::Write;
    for (m, yaml) in writes {
        let path = root.join(&m.agentic);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| OxyError::IOError(format!("create {}: {e}", m.agentic)))?;
        f.write_all(yaml.as_bytes())
            .map_err(|e| OxyError::IOError(format!("write {}: {e}", m.agentic)))?;
    }
    for (m, _) in writes {
        std::fs::remove_file(root.join(&m.classic))
            .map_err(|e| OxyError::IOError(format!("remove {}: {e}", m.classic)))?;
    }
    for e in edits {
        std::fs::write(root.join(&e.rel_path), &e.content)
            .map_err(|err| OxyError::IOError(format!("write {}: {err}", e.rel_path)))?;
    }
    Ok(())
}

fn topic_index(files: &[DiscoveredFile]) -> TopicIndex {
    let mut index = TopicIndex::new();
    for f in files.iter().filter(|f| f.kind == FileKind::SemanticTopic) {
        let file = f.rel_path.rsplit('/').next().unwrap_or(&f.rel_path);
        if let Some(stem) = file.strip_suffix(".topic.yml") {
            index
                .entry(stem.to_string())
                .or_default()
                .push(f.rel_path.clone());
        }
    }
    index
}

/// Ids existing agentic agents answer to: their `name:`, else their stem.
fn existing_agent_ids(files: &[DiscoveredFile]) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for f in files.iter().filter(|f| f.kind == FileKind::AgenticAgent) {
        let name = std::fs::read_to_string(&f.abs_path)
            .ok()
            .and_then(|c| serde_yaml::from_str::<serde_yaml::Value>(&c).ok())
            .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_owned));
        let id = name.unwrap_or_else(|| agent_id(&f.rel_path).to_string());
        ids.insert(id, f.rel_path.clone());
    }
    ids
}

fn agentic_path(classic: &str) -> String {
    let bare = classic.strip_suffix(".agent.yml").unwrap_or(classic);
    format!("{bare}.agentic.yml")
}

fn agent_id(agentic: &str) -> &str {
    let file = agentic.rsplit('/').next().unwrap_or(agentic);
    file.strip_suffix(".agentic.yml").unwrap_or(file)
}
