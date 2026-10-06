//! Markdown context documents: the one kind an extension alone does not find.
//!
//! An analytics agent names its grounding files by glob:
//!
//! ```yaml
//! context:
//!   - ./semantics/**/*
//!   - ./docs/*.md
//! ```
//!
//! Views, topics, automations and `.sql` compile wherever they sit, so a run on
//! a pod with no working copy finds them in the revision. Markdown did not:
//! `.md` is also a README, a changelog and a custom app's docs, and compiling
//! every one would store all of that on each compile to serve the few an agent
//! reads.
//!
//! So a document compiles when, and only when, some agent's `context:` reaches
//! it. [`ContextPatterns`] is the single definition of "reaches", and
//! [`discover`] the single walk. Three callers have to agree, and they agree by
//! sharing these two rather than by restating them:
//!
//! * the compile walker, with the union of every agent's patterns, deciding
//!   what a revision carries ([`reachable_from_agents`]);
//! * the reader of that revision, with ONE agent's patterns, picking that
//!   agent's documents back out of the union ([`ContextPatterns::position`]);
//! * the working-copy arm, with one agent's patterns, on a node that reads
//!   files ([`discover`]).
//!
//! # What "reaches" means
//!
//! The run used to hand each pattern to `glob::glob` under the workspace root
//! and keep the matches ending `.md`. This keeps that meaning, with three
//! deliberate narrowings, all of them things a revision cannot hold:
//!
//! * An absolute pattern, or one with a `..` component, names a file outside
//!   the workspace. It used to be read off the host. It reaches nothing now.
//! * The two workspace skip rules apply, as they do to every compiled kind: a
//!   path under a dot-prefixed, `target`, `node_modules`, `dist` or `build`
//!   directory is pruned, and a file whose NAME contains `.test.` is a
//!   fixture. `./**/*.md` used to pull in every README under `node_modules`.
//! * A directory symlink is not followed.
//!
//! Patterns are relative to the WORKSPACE ROOT, whatever directory the
//! `.agentic.yml` sits in. That is what the run has always done (it resolves
//! against the context root), and it is why a compile can evaluate them
//! without knowing which agent asked.

use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};
use tracing::{debug, warn};

use crate::walker::{DiscoveredFile, FileKind, is_skipped};

/// The first `revisions.schema_version` whose revisions carry context
/// documents.
///
/// A reader needs this because an empty table has two meanings. A revision
/// compiled at or after this version with no rows has no documents. One
/// compiled before it was never asked, and reading its absence as "none" would
/// run an agent without its documents and report success.
pub const SINCE_SCHEMA_VERSION: i32 = 2;

const _: () = assert!(crate::compile::CURRENT_SCHEMA_VERSION >= SINCE_SCHEMA_VERSION);

const DOCUMENT_SUFFIX: &str = ".md";

/// `*` and `?` stay inside one path component, as they do when `glob::glob`
/// walks a directory tree. Case-sensitive, and a leading dot is not special:
/// both are that function's defaults.
const MATCH: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// An agent's `context:` globs, reduced to the ones that can name a file inside
/// the workspace, in the order the agent listed them.
#[derive(Debug, Clone, Default)]
pub struct ContextPatterns {
    patterns: Vec<Pattern>,
}

impl ContextPatterns {
    pub fn new<I, S>(raw: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            patterns: raw
                .into_iter()
                .filter_map(|pattern| workspace_pattern(pattern.as_ref()))
                .collect(),
        }
    }

    /// Whether any pattern could match a markdown file at all.
    ///
    /// `./semantics/*.view.yml` cannot, and an agent whose patterns all look
    /// like that has no documents wherever it runs. Answering that here, before
    /// anything is read, is what keeps such an agent from being told to wait
    /// for a recompile it does not need.
    ///
    /// Conservative in one direction only: it may say yes for a pattern that
    /// turns out to match nothing, never no for one that could match.
    pub fn may_reach_documents(&self) -> bool {
        self.patterns
            .iter()
            .any(|pattern| may_name_a_document(pattern.as_str()))
    }

    /// The first pattern `rel_path` matches, by position.
    ///
    /// `Some` is "this agent reads this file". The index orders the documents
    /// the way the agent listed its patterns, which is the order the run has
    /// always injected them in.
    pub fn position(&self, rel_path: &str) -> Option<usize> {
        self.patterns
            .iter()
            .position(|pattern| pattern.matches_with(rel_path, MATCH))
    }
}

/// `raw` as a pattern over workspace-relative, `/`-separated paths, or `None`
/// when it cannot name a file in the workspace.
fn workspace_pattern(raw: &str) -> Option<Pattern> {
    if raw.starts_with('/') {
        debug!(
            pattern = raw,
            "context pattern is absolute; it reaches no workspace file"
        );
        return None;
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                debug!(
                    pattern = raw,
                    "context pattern leaves the workspace; it reaches no workspace file"
                );
                return None;
            }
            part => parts.push(part),
        }
    }
    // `docs/**` names directories, not the files in them: `glob::glob` yields
    // `docs` and its subdirectories for it and no file. `Pattern::matches`
    // would accept every path below `docs/`, so the difference is stated here.
    if parts.last().is_none_or(|last| *last == "**") {
        return None;
    }
    match Pattern::new(&parts.join("/")) {
        Ok(pattern) => Some(pattern),
        Err(error) => {
            // The run's own resolution of the same pattern reports this; the
            // agent fails to build there, with the pattern named.
            debug!(pattern = raw, %error, "context pattern does not parse");
            None
        }
    }
}

/// Whether some file name ending `.md` can match the pattern's last component.
fn may_name_a_document(pattern: &str) -> bool {
    let last = pattern.rsplit('/').next().unwrap_or(pattern);
    match last.rfind(['*', '?', ']']) {
        None => last.ends_with(DOCUMENT_SUFFIX),
        // Everything after the last wildcard is literal, so a matching name
        // ends with it. A name can end with both it and `.md` only when one is
        // a suffix of the other.
        Some(at) => {
            let literal = &last[at + 1..];
            literal.ends_with(DOCUMENT_SUFFIX) || DOCUMENT_SUFFIX.ends_with(literal)
        }
    }
}

/// A document's bytes as the text an agent is given, on either arm.
///
/// Lossy UTF-8, then NUL bytes removed. Postgres `TEXT` cannot hold a NUL, so
/// a revision could never carry one verbatim; the choices were to fail the
/// compile over it or to drop it. Failing is out of proportion: one odd `.md`
/// under a broad glob would stop every promotion of the workspace, for a byte
/// that means nothing in a prompt. Dropping it here, in the one function both
/// the compiler and the working-copy read call, keeps the two arms handing a
/// run the same text for the same file.
pub fn document_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\0') {
        text.replace('\0', "")
    } else {
        text.into_owned()
    }
}

/// `name` is a markdown file by the test the run has always applied.
fn is_document_name(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| ext == "md")
}

/// A markdown file some pattern reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFile {
    /// Workspace-relative, `/`-separated. The key a compiled row carries.
    pub rel_path: String,
    pub abs_path: PathBuf,
}

/// Every markdown file under `root` that `patterns` reach, ordered by the
/// first pattern that matches and then by path.
///
/// An unreadable `root` is an error: the caller is asking about a workspace,
/// and "could not look" must not come back as "found none". An unreadable
/// directory below it is skipped with a warning, as the glob walk the other
/// kinds use does.
pub fn discover(root: &Path, patterns: &ContextPatterns) -> std::io::Result<Vec<DocumentFile>> {
    if !patterns.may_reach_documents() {
        return Ok(Vec::new());
    }
    let mut walk = Walk {
        patterns,
        found: Vec::new(),
    };
    walk.directory(root, "", std::fs::read_dir(root)?);
    let mut found = walk.found;
    found.sort_by(|a, b| (a.0, &a.1.rel_path).cmp(&(b.0, &b.1.rel_path)));
    Ok(found.into_iter().map(|(_, file)| file).collect())
}

/// One pass over the tree. `found` pairs each document with the position of
/// the pattern that claimed it.
struct Walk<'a> {
    patterns: &'a ContextPatterns,
    found: Vec<(usize, DocumentFile)>,
}

impl Walk<'_> {
    fn directory(&mut self, dir: &Path, rel_dir: &str, entries: std::fs::ReadDir) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let rel_path = match rel_dir {
                "" => name.clone(),
                parent => format!("{parent}/{name}"),
            };
            let path = dir.join(&name);
            // `file_type` does not follow a symlink, so a directory symlink is
            // neither descended into nor mistaken for a file.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                self.descend(&path, &rel_path, &name);
            } else if is_document_name(&name) && path.is_file() {
                self.file(path, rel_path, &name);
            }
        }
    }

    fn descend(&mut self, path: &Path, rel_path: &str, name: &str) {
        if is_skipped(name) {
            // One line per directory, not per file: this prunes before it
            // descends, which is what keeps it out of `node_modules`.
            debug!(rel_path, "context documents: pruning a skipped directory");
            return;
        }
        match std::fs::read_dir(path) {
            Ok(entries) => self.directory(path, rel_path, entries),
            Err(error) => warn!(rel_path, %error, "context documents: directory not readable"),
        }
    }

    fn file(&mut self, abs_path: PathBuf, rel_path: String, name: &str) {
        if name.contains(".test.") {
            debug!(rel_path, "context documents: dropping a test fixture");
            return;
        }
        if let Some(position) = self.patterns.position(&rel_path) {
            self.found
                .push((position, DocumentFile { rel_path, abs_path }));
        }
    }
}

/// The documents a compile has to carry: everything any agent in the workspace
/// reaches. `agents` are the `.agentic.yml` files the walker already found.
pub(crate) fn reachable_from_agents(
    root: &Path,
    agents: &[DiscoveredFile],
) -> std::io::Result<Vec<DiscoveredFile>> {
    let patterns = ContextPatterns::new(agents.iter().flat_map(agent_context));
    Ok(discover(root, &patterns)?
        .into_iter()
        .map(|file| DiscoveredFile {
            rel_path: file.rel_path,
            abs_path: file.abs_path,
            kind: FileKind::ContextDocument,
        })
        .collect())
}

/// One agent file's `context:` list.
///
/// Lenient on purpose. A file that cannot be read or parsed contributes no
/// patterns here and fails the compile on its own a moment later, when it is
/// compiled as an agent, with its own message.
fn agent_context(agent: &DiscoveredFile) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(&agent.abs_path) else {
        return Vec::new();
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return Vec::new();
    };
    value
        .get("context")
        .and_then(|context| context.as_sequence())
        .map(|patterns| {
            patterns
                .iter()
                .filter_map(|pattern| pattern.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "context_documents_tests.rs"]
mod tests;
