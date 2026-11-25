//! Rewrite references to translated classic agents.
//!
//! A reference is a YAML scalar on one line: an `agent_ref:` value (by path,
//! by path minus extension, by stem or by the classic `name:`), any value that
//! is a `.agent.yml` path (a test `target:`, a route), or a glob over
//! `.agent.yml` files (`config.yml` `mcp.tools`). The rewrite is textual and
//! per line, so comments and layout survive. A new value is written only when
//! [`agent_ref_candidates`] resolves it to the translated agent, the rule the
//! compile and runtime use, so a ref by stem keeps resolving by stem.

use std::path::Path;

use oxy_compile::agent_ref_candidates;
use serde::Serialize;

/// A classic file and where it went (or would go).
#[derive(Debug, Clone)]
pub(super) struct AgentMove {
    pub classic: String,
    pub agentic: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RefRewrite {
    pub file: String,
    pub line: usize,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockedRef {
    pub file: String,
    pub line: usize,
    pub value: String,
    pub reason: String,
}

/// One file's new content and the rewrites in it.
pub(super) struct FileEdit {
    pub rel_path: String,
    pub content: String,
}

/// What [`rewrite_file`] found in one file.
#[derive(Default)]
pub(super) struct FileRefs {
    pub content: Option<String>,
    pub rewritten: Vec<RefRewrite>,
    pub blocked: Vec<BlockedRef>,
}

/// Rewrite every reference in `content` (the file at `rel_path`). `moved` are
/// translated files; `stuck` are classic files left in place, reported when
/// something names them by path.
pub(super) fn rewrite_file(
    rel_path: &str,
    content: &str,
    moved: &[AgentMove],
    stuck: &[String],
) -> FileRefs {
    let mut out = FileRefs::default();
    let mut lines = Vec::new();
    for (idx, line) in content.split('\n').enumerate() {
        let Some(s) = scalar(line) else {
            lines.push(line.to_string());
            continue;
        };
        let value = &line[s.start..s.end];
        if let Some(new) = rewrite_value(value, s.key == Some("agent_ref"), moved) {
            out.rewritten.push(RefRewrite {
                file: rel_path.to_string(),
                line: idx + 1,
                from: value.to_string(),
                to: new.clone(),
            });
            lines.push(format!("{}{new}{}", &line[..s.start], &line[s.end..]));
            continue;
        }
        if let Some(classic) = stuck.iter().find(|c| names_path(value, c)) {
            out.blocked.push(BlockedRef {
                file: rel_path.to_string(),
                line: idx + 1,
                value: value.to_string(),
                reason: format!("{classic} was not translated"),
            });
        }
        lines.push(line.to_string());
    }
    if !out.rewritten.is_empty() {
        out.content = Some(lines.join("\n"));
    }
    out
}

/// The value a reference should now read, or `None` to leave it.
pub(super) fn rewrite_value(
    value: &str,
    is_agent_ref: bool,
    moved: &[AgentMove],
) -> Option<String> {
    let v = value.trim().trim_start_matches("./");
    if v.contains('*') {
        return rewrite_glob(value, moved);
    }
    let classic_ext = classic_extension(v);
    // Already resolves to a translated agent (`local`, `agents/local`).
    if classic_ext.is_none() && moved.iter().any(|m| resolves(v, m)) {
        return None;
    }
    let target = moved.iter().find(|m| names_classic(v, m, is_agent_ref))?;
    // Keep the ref's own spelling where it still resolves: `x.agent.yml` by
    // stem becomes `x.agentic.yml`, a path becomes the agentic path.
    let new = match classic_ext {
        Some(ext) if !v.contains('/') => format!("{}.agentic.yml", &v[..v.len() - ext.len()]),
        _ => target.agentic.clone(),
    };
    let new = if resolves(&new, target) {
        new
    } else {
        target.agentic.clone()
    };
    let prefix = &value[..value.len() - value.trim_start().len()];
    let dot = if value.trim_start().starts_with("./") && new.contains('/') {
        "./"
    } else {
        ""
    };
    Some(format!("{prefix}{dot}{new}"))
}

/// Whether `value`, as an `agent_ref`, resolves to the moved agent.
fn resolves(value: &str, m: &AgentMove) -> bool {
    let known = agent_ref_candidates(&m.agentic);
    agent_ref_candidates(value)
        .iter()
        .any(|c| known.contains(c))
}

/// Whether `v` names the classic file: by path, by stem with its extension,
/// or (for an `agent_ref`) by path minus extension, stem, or classic `name:`.
fn names_classic(v: &str, m: &AgentMove, is_agent_ref: bool) -> bool {
    if names_path(v, &m.classic) {
        return true;
    }
    if !is_agent_ref {
        return false;
    }
    let bare = strip_classic(&m.classic);
    let stem = bare.rsplit('/').next().unwrap_or(bare);
    v == bare || v == stem || m.name.as_deref() == Some(v)
}

/// `v` is the classic path, or its file name when `v` has no directory.
fn names_path(v: &str, classic: &str) -> bool {
    let v = v.trim().trim_start_matches("./");
    if v == classic {
        return true;
    }
    let file = classic.rsplit('/').next().unwrap_or(classic);
    classic_extension(v).is_some() && !v.contains('/') && v == file
}

/// `agents/*.agent.yml` → `agents/*.agentic.yml` when it matches a translated
/// file. A glob only over files left in place is left too.
fn rewrite_glob(value: &str, moved: &[AgentMove]) -> Option<String> {
    let v = value.trim().trim_start_matches("./");
    let ext = classic_extension(v)?;
    let pattern = glob::Pattern::new(v).ok()?;
    if !moved.iter().any(|m| pattern.matches(&m.classic)) {
        return None;
    }
    let cut = value.len() - ext.len();
    Some(format!("{}.agentic.yml", &value[..cut]))
}

fn classic_extension(v: &str) -> Option<&'static str> {
    [".agent.yml", ".agent.yaml"]
        .into_iter()
        .find(|e| v.ends_with(e))
}

fn strip_classic(path: &str) -> &str {
    classic_extension(path).map_or(path, |e| &path[..path.len() - e.len()])
}

/// The byte range of a line's single scalar value, and its key if any.
struct Scalar<'a> {
    key: Option<&'a str>,
    start: usize,
    end: usize,
}

/// `  - key: "value"  # c`, `key: value`, `- value`. Not block scalars, flow
/// collections or comments.
fn scalar(line: &str) -> Option<Scalar<'_>> {
    let mut i = line.len() - line.trim_start().len();
    if line[i..].starts_with('#') {
        return None;
    }
    if line[i..].starts_with("- ") {
        i += 2;
        i += line[i..].len() - line[i..].trim_start().len();
    }
    let mut key = None;
    let ident = line[i..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(line.len() - i);
    let after = &line[i + ident..];
    if ident > 0 && (after.starts_with(": ") || after == ":") {
        key = Some(&line[i..i + ident]);
        i += ident + 1;
        i += line[i..].len() - line[i..].trim_start().len();
    }
    value_range(line, i).map(|(start, end)| Scalar { key, start, end })
}

fn value_range(line: &str, i: usize) -> Option<(usize, usize)> {
    let rest = &line[i..];
    let first = rest.chars().next()?;
    if first == '"' || first == '\'' {
        let close = rest[1..].find(first)?;
        return Some((i + 1, i + 1 + close));
    }
    if "|>{[&*!#".contains(first) {
        return None;
    }
    let end = rest.find(" #").unwrap_or(rest.len());
    let len = rest[..end].trim_end().len();
    (len > 0).then_some((i, i + len))
}

/// Every `.yml` / `.yaml` file under `root` the walker would not skip, as
/// workspace-relative paths, sorted.
pub(super) fn yaml_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if path.is_dir() {
            if !oxy_compile::walker::is_skipped(&rel) {
                walk(root, &path, out);
            }
        } else if rel.ends_with(".yml") || rel.ends_with(".yaml") {
            out.push(rel);
        }
    }
}
