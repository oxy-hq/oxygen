//! Verified queries: hand-written `.sql` files in the agent's `context:` that
//! open with a `/* oxy: … */` header. The FSM's rule decides which files count
//! ([`parse_oxy_comment_block`] reads a `description:` or `database:`); a
//! `.sql` file without one is only an example. The system prompt lists them
//! with their `embed:` questions, and `query {"verified": "<id>"}` runs one as
//! written, tagged `verified_sql`.

use agentic_core::react::ToolFailure;
use agentic_core::subrun::parse_oxy_comment_block;
use serde::Deserialize;
use serde_json::Value;

use super::files::{FileKind, WorkspaceFiles, stem};

/// Ids offered as candidates when a `verified` id names none.
const MAX_CANDIDATES: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedQuery {
    /// The file's path in the snapshot: the id `query` takes.
    pub id: String,
    /// The header's `database:`, the connector the SQL is written for.
    /// `None` runs it where a raw query with no `database` would run.
    pub database: Option<String>,
    pub description: Option<String>,
    /// The header's `embed:` questions: what the query answers.
    pub questions: Vec<String>,
    /// What `query` runs: the file with its leading header removed.
    pub sql: String,
}

/// The agent's verified queries, sorted by id.
#[derive(Debug, Default)]
pub struct VerifiedQueries {
    entries: Vec<VerifiedQuery>,
}

impl VerifiedQueries {
    /// The verified queries among the snapshot's `.sql` files.
    pub fn from_files(files: &WorkspaceFiles) -> Self {
        let entries = files
            .entries()
            .filter(|e| e.kind == FileKind::Sql)
            .filter_map(|e| verified_query(&e.path, &e.content))
            .collect();
        Self { entries }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn as_slice(&self) -> &[VerifiedQuery] {
        &self.entries
    }

    /// An exact id, else a bare name (`total_number_of_store`) matching
    /// exactly one file stem.
    pub fn get(&self, id: &str) -> Option<&VerifiedQuery> {
        let id = id.trim();
        if let Some(q) = self.entries.iter().find(|q| q.id == id) {
            return Some(q);
        }
        let mut hits = self.entries.iter().filter(|q| stem(&q.id) == id);
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    /// The query `query`'s `verified` argument names. Refused: a call that
    /// also carries `sql` or `semantic` (the SQL runs as written, never
    /// edited), an id that is not a string, and one that names no query.
    pub(crate) fn resolve(&self, arg: &Value, args: &Value) -> Result<&VerifiedQuery, ToolFailure> {
        let edited = ["sql", "semantic"]
            .iter()
            .any(|k| args.get(*k).is_some_and(|v| !v.is_null()));
        if edited {
            return Err(failure(
                "`verified` runs the query's SQL as written; give no `sql` or `semantic` with it",
                Some("to change the SQL, send it as `sql` alone: it is then your own query"),
            ));
        }
        let id = arg.as_str().ok_or_else(|| {
            failure(
                "`verified` must be a string: the id of a verified query",
                None,
            )
        })?;
        self.get(id).ok_or_else(|| self.unknown(id))
    }

    fn unknown(&self, id: &str) -> ToolFailure {
        if self.entries.is_empty() {
            return failure(
                "this agent has no verified queries",
                Some("use `sql` or `semantic`"),
            );
        }
        let mut scored: Vec<(f64, &str)> = self
            .entries
            .iter()
            .map(|q| (strsim::jaro_winkler(id, &q.id), q.id.as_str()))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(b.1)));
        ToolFailure {
            error: format!("no verified query {id}"),
            hint: Some("pass an id from the verified queries in the system prompt".into()),
            candidates: scored
                .into_iter()
                .take(MAX_CANDIDATES)
                .map(|(_, p)| p.to_owned())
                .collect(),
        }
    }
}

/// `content` as a verified query with id `path`; `None` when its header does
/// not opt it in.
pub fn verified_query(path: &str, content: &str) -> Option<VerifiedQuery> {
    let block = parse_oxy_comment_block(content)?;
    Some(VerifiedQuery {
        id: path.to_owned(),
        database: block.database,
        description: block.description,
        questions: embed_questions(content),
        sql: strip_header(content).to_owned(),
    })
}

/// Byte range of the first `/* … */` block: the one
/// [`parse_oxy_comment_block`] reads.
fn first_block(content: &str) -> Option<(usize, usize)> {
    let start = content.find("/*")?;
    let end = start + 2 + content[start + 2..].find("*/")? + 2;
    Some((start, end))
}

/// The header's `embed:` list; empty when it has none or it is not a list of
/// strings. `parse_oxy_comment_block` does not carry it, and adding it there
/// would change which files the FSM treats as verified.
fn embed_questions(content: &str) -> Vec<String> {
    #[derive(Deserialize)]
    struct Inner {
        #[serde(default)]
        embed: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Header {
        oxy: Option<Inner>,
    }
    let Some((start, end)) = first_block(content) else {
        return Vec::new();
    };
    let inner = serde_yaml::from_str::<Header>(&content[start + 2..end - 2])
        .ok()
        .and_then(|h| h.oxy);
    inner
        .map(|i| i.embed)
        .unwrap_or_default()
        .into_iter()
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty())
        .collect()
}

/// The SQL after the leading header block (and the whitespace after it). A
/// header that something other than whitespace precedes is left in place.
fn strip_header(content: &str) -> &str {
    match first_block(content) {
        Some((start, end)) if content[..start].trim().is_empty() => content[end..].trim_start(),
        _ => content,
    }
}

fn failure(error: &str, hint: Option<&str>) -> ToolFailure {
    ToolFailure {
        error: error.to_owned(),
        hint: hint.map(str::to_owned),
        candidates: Vec::new(),
    }
}
