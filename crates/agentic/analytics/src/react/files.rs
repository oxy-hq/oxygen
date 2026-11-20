//! An in-memory snapshot of the files an agent's `context:` globs resolve.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::config::resolve_context_pattern;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    View,
    Topic,
    Sql,
    Doc,
}

impl FileKind {
    /// Kind from a file name; anything unrecognized is a `Doc`.
    pub fn from_name(name: &str) -> Self {
        if name.ends_with(".view.yml") || name.ends_with(".view.yaml") {
            Self::View
        } else if name.ends_with(".topic.yml") || name.ends_with(".topic.yaml") {
            Self::Topic
        } else if name.ends_with(".sql") {
            Self::Sql
        } else {
            Self::Doc
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Topic => "topic",
            Self::Sql => "sql",
            Self::Doc => "doc",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "view" => Some(Self::View),
            "topic" => Some(Self::Topic),
            "sql" => Some(Self::Sql),
            "doc" => Some(Self::Doc),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    /// Relative to the base dir, `/`-separated.
    pub path: String,
    pub kind: FileKind,
    pub content: String,
}

/// The files `search` and `read` can reach, sorted by path.
#[derive(Debug, Default)]
pub struct WorkspaceFiles {
    entries: BTreeMap<String, FileEntry>,
}

impl WorkspaceFiles {
    pub fn from_entries(entries: Vec<FileEntry>) -> Self {
        Self {
            entries: entries.into_iter().map(|e| (e.path.clone(), e)).collect(),
        }
    }

    /// Reads every file the `globs` resolve (the same resolution
    /// `AgentConfig::resolve_context` uses), keeping only the kinds the FSM
    /// context keeps: views, topics, `.sql` and `.md`. `*.test.*` files are
    /// dropped. `docs` are host-resolved markdown with no path, named
    /// `docs/context_<n>.md`; `Some` replaces any `.md` the globs found (even
    /// when empty), `None` keeps them. A file whose real path is not under
    /// `base_dir` (a `..` escape or a symlink) is skipped, as is one over
    /// [`MAX_FILE_BYTES`].
    pub fn load(
        base_dir: &Path,
        globs: &[String],
        docs: Option<&[String]>,
    ) -> Result<Self, String> {
        let canon_base =
            std::fs::canonicalize(base_dir).map_err(|e| format!("{}: {e}", base_dir.display()))?;
        let mut entries = Vec::new();
        for pattern in globs {
            let paths = resolve_context_pattern(base_dir, pattern).map_err(|e| e.to_string())?;
            for path in paths {
                if let Some(entry) = load_one(base_dir, &canon_base, &path, docs.is_some())? {
                    entries.push(entry);
                }
            }
        }
        for (i, content) in docs.unwrap_or_default().iter().enumerate() {
            entries.push(FileEntry {
                path: format!("docs/context_{}.md", i + 1),
                kind: FileKind::Doc,
                content: content.clone(),
            });
        }
        Ok(Self::from_entries(entries))
    }

    pub fn paths(&self) -> Vec<(&str, FileKind)> {
        self.entries
            .values()
            .map(|e| (e.path.as_str(), e.kind))
            .collect()
    }

    pub fn get(&self, path: &str) -> Option<&FileEntry> {
        self.entries.get(path)
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &FileEntry> {
        self.entries.values()
    }
}

/// `path` relative to `base_dir`, `/`-separated; `None` when it is not
/// under `base_dir` or climbs with `..`.
fn relative_path(base_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base_dir).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Largest file the snapshot will hold.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Whether the FSM context would keep a file of this name.
fn admitted(name: &str) -> bool {
    !name.contains(".test.")
        && (FileKind::from_name(name) != FileKind::Doc || name.ends_with(".md"))
        && !name.ends_with(".automation.yml")
        && !name.ends_with(".automation.yaml")
        && !name.ends_with(".procedure.yml")
        && !name.ends_with(".procedure.yaml")
}

fn load_one(
    base_dir: &Path,
    canon_base: &Path,
    path: &Path,
    host_docs: bool,
) -> Result<Option<FileEntry>, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !admitted(name) || (host_docs && name.ends_with(".md")) {
        return Ok(None);
    }
    let Some(rel) = relative_path(base_dir, path) else {
        return Ok(None);
    };
    let Ok(real) = std::fs::canonicalize(path) else {
        return Ok(None);
    };
    if !real.starts_with(canon_base) || !real.is_file() {
        return Ok(None);
    }
    let len = std::fs::metadata(&real)
        .map_err(|e| format!("{rel}: {e}"))?
        .len();
    if len > MAX_FILE_BYTES {
        tracing::warn!(path = %rel, bytes = len, "react workspace snapshot skips an oversized file");
        return Ok(None);
    }
    let bytes = std::fs::read(&real).map_err(|e| format!("{rel}: {e}"))?;
    Ok(Some(FileEntry {
        kind: FileKind::from_name(name),
        path: rel,
        content: String::from_utf8_lossy(&bytes).into_owned(),
    }))
}
