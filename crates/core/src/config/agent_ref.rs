//! Resolving an agent reference to the one agent it names.
//!
//! Callers name an agent three ways: by its workspace-relative path
//! (`agents/sales/foo.agentic.yml`), by that path without the extension
//! (`agents/sales/foo`), or by its compile key (`name:`, else the path minus
//! the extension, `oxy_compile::compile::derive_name_from_path`). The lookup
//! used to strip every ref to its file stem (`foo`) and ask for that name,
//! which is the compiler's key only for a root-level agent with no `name:`:
//! every other agent missed its row (gap G1 of the classic-agent retirement
//! memo).
//!
//! The order is the memo's: an exact `file_path` first, then the ref with
//! `.agentic.yml` (or `.agentic.yaml`) appended, then the compile key. A
//! caller passing a path gets that file. A bare stem gets the root-level
//! `{stem}.agentic.yml`, which is what the disk fallback read before.
//!
//! Last, a bare stem (no `/`) that matched none of those names the one agent
//! whose file is called `{stem}.agentic.yml` in any directory: the spelling the
//! old stem lookup accepted, and one the compiler's `agent_ref` check accepts,
//! so the compiler never calls "resolves" something this answers 404 for. Two
//! such files are ambiguous, and that is said rather than one picked.
//!
//! The spellings themselves (`./` dropped, extension stripped, stem) are
//! `oxy_compile::agent_ref_spellings`, the normaliser the compiler's own
//! `agent_ref` check uses; this module only orders the lookups.
//!
//! Whether `name:` should keep overriding the key at all is the memo's open
//! question Q4; this does not change what the compiler writes.

use uuid::Uuid;

use oxy_compile::agent_ref_spellings;
use oxy_shared::errors::OxyError;

use super::artifacts::{AgentEntry, AgentRef, ArtifactError};

pub(super) use oxy_compile::AGENT_EXTENSIONS as EXTENSIONS;

/// The ref with a leading `./` dropped: the form every key is stored in.
fn normalised(agent_ref: &str) -> &str {
    agent_ref_spellings(agent_ref).full
}

/// The `file_path`s `agent_ref` may name, in lookup order.
pub(super) fn candidate_paths(agent_ref: &str) -> Vec<String> {
    let path = normalised(agent_ref);
    if path.is_empty() {
        return Vec::new();
    }
    let mut paths = vec![path.to_string()];
    if !EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
        paths.extend(EXTENSIONS.iter().map(|ext| format!("{path}{ext}")));
    }
    paths
}

/// The compile key `agent_ref` may name: the ref without its extension. For
/// a nameless agent that is the same row a path matched; it differs only for
/// an agent whose `name:` overrides its path.
pub(super) fn candidate_name(agent_ref: &str) -> &str {
    agent_ref_spellings(agent_ref).bare
}

/// The file stem a bare ref may name, or `None` for a ref with a directory.
fn candidate_stem(agent_ref: &str) -> Option<&str> {
    let stem = candidate_name(agent_ref);
    (!stem.is_empty() && !stem.contains('/')).then_some(stem)
}

/// The file's last segment without its extension: `agents/sales/foo` for
/// `agents/sales/foo.agentic.yml` is `foo`.
fn file_stem(file_path: &str) -> &str {
    agent_ref_spellings(file_path).stem
}

/// The one entry whose file is `{stem}.agentic.yml` in any directory. Two are
/// ambiguous: a permanent fault of the ref, never a guess.
pub(super) fn by_stem<'a>(
    entries: &'a [AgentEntry],
    stem: &str,
) -> Result<Option<&'a AgentEntry>, ArtifactError> {
    let mut matches = entries.iter().filter(|e| file_stem(&e.file_path) == stem);
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    let rest: Vec<&str> = matches.map(|e| e.file_path.as_str()).collect();
    if rest.is_empty() {
        return Ok(Some(first));
    }
    Err(ArtifactError::Config(OxyError::ConfigurationError(
        format!(
            "agent `{stem}` is ambiguous: it names {} and {}; use the path",
            first.file_path,
            rest.join(", ")
        ),
    )))
}

/// The listing entry `agent_ref` names, by the same order the compiled read
/// uses. The working-copy arm's half of the lookup.
pub(super) fn pick<'a>(
    entries: &'a [AgentEntry],
    agent_ref: &str,
) -> Result<Option<&'a AgentEntry>, ArtifactError> {
    let exact = candidate_paths(agent_ref)
        .iter()
        .find_map(|path| entries.iter().find(|e| &e.file_path == path))
        .or_else(|| {
            let name = candidate_name(agent_ref);
            entries.iter().find(|e| !name.is_empty() && e.name == name)
        });
    match (exact, candidate_stem(agent_ref)) {
        (Some(entry), _) => Ok(Some(entry)),
        (None, Some(stem)) => by_stem(entries, stem),
        (None, None) => Ok(None),
    }
}

/// The compiled row `agent_ref` names at `revision_id`. A miss is `Ok(None)`
/// and a failed read is `Err`: only the second is worth a retry.
pub(super) async fn compiled(
    revision_id: Uuid,
    agent_ref: &str,
) -> Result<Option<AgentRef>, ArtifactError> {
    for path in candidate_paths(agent_ref) {
        if let Some(found) = super::compiled::resolve_agent_by_path_at(revision_id, &path).await? {
            return Ok(Some(found));
        }
    }
    let name = candidate_name(agent_ref);
    if name.is_empty() {
        return Ok(None);
    }
    if let Some(found) = super::compiled::resolve_agent_at(revision_id, name).await? {
        return Ok(Some(found));
    }
    let Some(stem) = candidate_stem(agent_ref) else {
        return Ok(None);
    };
    // Only on a miss: the listing carries no definitions, so the one row it
    // picks is read by its path.
    let agents = super::compiled::list_agents_at(revision_id).await?;
    match by_stem(&agents, stem)? {
        Some(entry) => {
            super::compiled::resolve_agent_by_path_at(revision_id, &entry.file_path).await
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, file_path: &str) -> AgentEntry {
        AgentEntry {
            name: name.into(),
            file_path: file_path.into(),
            model_ref: None,
            timezone: None,
        }
    }

    /// The rows as the compiler keys them: a nameless agent by its path minus
    /// the extension, a named one by its `name:`.
    fn workspace() -> Vec<AgentEntry> {
        vec![
            entry("agents/sales/foo", "agents/sales/foo.agentic.yml"),
            entry("foo", "foo.agentic.yml"),
            entry("revenue", "agents/finance/rev.agentic.yml"),
        ]
    }

    fn picked(agent_ref: &str) -> Option<String> {
        pick(&workspace(), agent_ref)
            .expect("unambiguous")
            .map(|e| e.file_path.clone())
    }

    /// G1: a nameless agent in a subdirectory is found by every form of its
    /// path, and never confused with a root-level agent sharing its stem.
    #[test]
    fn a_nameless_agent_in_a_subdirectory_is_found_by_its_path() {
        for agent_ref in [
            "agents/sales/foo.agentic.yml",
            "./agents/sales/foo.agentic.yml",
            "agents/sales/foo",
        ] {
            assert_eq!(
                picked(agent_ref).as_deref(),
                Some("agents/sales/foo.agentic.yml"),
                "{agent_ref}"
            );
        }
    }

    /// A bare stem is the root-level file, as the disk fallback read it.
    #[test]
    fn a_bare_stem_is_the_root_level_agent() {
        assert_eq!(picked("foo").as_deref(), Some("foo.agentic.yml"));
        assert_eq!(
            picked("foo.agentic.yml").as_deref(),
            Some("foo.agentic.yml")
        );
    }

    /// `name:` still answers when no path matches: today's behaviour, kept
    /// while Q4 is open.
    #[test]
    fn a_name_override_still_resolves_after_the_paths() {
        assert_eq!(
            picked("revenue").as_deref(),
            Some("agents/finance/rev.agentic.yml")
        );
        assert_eq!(
            picked("agents/finance/rev").as_deref(),
            Some("agents/finance/rev.agentic.yml")
        );
    }

    /// The stem of a path that names nothing is not looked up: that would
    /// answer with a different file than the one asked for.
    #[test]
    fn a_path_that_names_nothing_is_a_miss_not_a_stem_match() {
        assert_eq!(picked("agents/other/foo.agentic.yml"), None);
        assert_eq!(picked("agents/missing"), None);
        assert_eq!(picked("missing"), None);
        assert_eq!(picked(""), None);
    }

    /// A bare stem with no root-level file and no such name finds the one
    /// agent of that file name in any directory: what the compiler's
    /// `agent_ref` check accepts.
    #[test]
    fn a_bare_stem_finds_the_one_agent_of_that_file_name() {
        assert_eq!(
            picked("rev").as_deref(),
            Some("agents/finance/rev.agentic.yml")
        );
        let nested = vec![entry("agents/sales/foo", "agents/sales/foo.agentic.yml")];
        let found = pick(&nested, "foo").expect("unambiguous");
        assert_eq!(
            found.map(|e| e.file_path.as_str()),
            Some("agents/sales/foo.agentic.yml")
        );
    }

    /// Two files of that name: said, not guessed, and not a retry.
    #[test]
    fn a_bare_stem_naming_two_files_is_ambiguous() {
        let two = vec![
            entry("agents/sales/foo", "agents/sales/foo.agentic.yml"),
            entry("agents/ops/foo", "agents/ops/foo.agentic.yml"),
        ];
        let err = pick(&two, "foo").expect_err("ambiguous");
        assert!(!err.retryable(), "{err}");
        assert!(err.to_string().contains("ambiguous"), "{err}");
    }

    #[test]
    fn candidates_are_tried_path_first() {
        assert_eq!(
            candidate_paths("agents/foo"),
            vec![
                "agents/foo",
                "agents/foo.agentic.yml",
                "agents/foo.agentic.yaml"
            ]
        );
        assert_eq!(
            candidate_paths("agents/foo.agentic.yml"),
            vec!["agents/foo.agentic.yml"]
        );
        assert_eq!(candidate_name("agents/foo.agentic.yml"), "agents/foo");
        assert_eq!(candidate_name("./foo"), "foo");
    }
}
