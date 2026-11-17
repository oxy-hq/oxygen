//! What a revision is made of, by content: one entry per compiled file with the
//! SHA-256 of its text, the text itself for the kinds a reader parses, and one
//! fingerprint per kind.
//!
//! The shape Bazel's content-addressable store and git's trees share: bodies
//! stored once by hash (`compiled_sources`), and a small manifest per revision
//! that names them (`revision_manifest`). A push that changes one file adds one
//! body; every other entry points at text the workspace already has. Per-kind
//! fingerprints (`revision_fingerprints`) answer "did the semantic model change
//! between these two revisions" without reading a body — the comparison a data
//! release's diff, and the semantic gate's ratchet, are built on.
//!
//! Pure: no I/O. `compile.rs` builds the entries while it reads each file, and
//! `writer.rs` persists them in the finalise transaction.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::walker::FileKind;

/// The kinds whose TEXT is stored, not only hashed: the semantic views and
/// topics, which `oxy::config::scan` parses from the pushed text. Every other
/// kind is read from its parsed row, so storing its text would only add a copy
/// — and for `config.yml` it would undo secret redaction.
const TEXT_KINDS: &[&str] = &["view", "topic"];

/// The kinds the `semantic` fingerprint covers.
const SEMANTIC_KINDS: &[&str] = &["view", "topic"];

/// One compiled file, by content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    pub file_path: String,
    /// [`kind_label`] of the file's kind.
    pub kind: &'static str,
    /// Lowercase hex SHA-256 of the file's text as the compiler read it.
    pub content_sha256: String,
    /// The text, for [`TEXT_KINDS`] only. `None` also when the text holds a
    /// NUL, which Postgres `TEXT` cannot store: the reader then falls back to
    /// the parsed row, as it does for a revision compiled before this existed.
    pub text: Option<String>,
}

impl SourceEntry {
    /// The entry for `file` of `kind` with `content`, or `None` for a kind that
    /// produces no row (a retired agent).
    pub fn new(file_path: &str, kind: FileKind, content: &str) -> Option<Self> {
        let kind = kind_label(kind)?;
        let keep_text = TEXT_KINDS.contains(&kind) && !content.contains('\0');
        Some(Self {
            file_path: file_path.to_string(),
            kind,
            content_sha256: sha256_hex(content.as_bytes()),
            text: keep_text.then(|| content.to_string()),
        })
    }
}

/// The stable name a kind is recorded under. These are stored, so they are a
/// contract: rename a variant freely, never its label.
pub fn kind_label(kind: FileKind) -> Option<&'static str> {
    Some(match kind {
        FileKind::Config => "config",
        FileKind::AgenticAgent => "agent",
        FileKind::SemanticView => "view",
        FileKind::SemanticTopic => "topic",
        FileKind::Automation(_) => "automation",
        FileKind::VerifiedQuery => "verified_query",
        FileKind::SchemaMigration => "schema_migration",
        FileKind::AirwayPipeline => "pipeline",
        FileKind::MonitorConfig => "monitor_config",
        FileKind::ReconcileConfig => "reconcile_config",
        FileKind::WorldModelConfig => "world_model_config",
        FileKind::Simulation => "simulation",
        FileKind::ContextDocument => "context_document",
        FileKind::RetiredAgent => return None,
    })
}

/// One kind's hash over a revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// A [`kind_label`], or `semantic` (views and topics together), or `all`.
    pub kind: String,
    pub fingerprint: String,
    pub file_count: usize,
    /// The semantic gate's verdict, on the `semantic` fingerprint only.
    pub validated: Option<bool>,
}

/// A fingerprint per kind present, plus `semantic` and `all`, each a SHA-256
/// over the kind's entries sorted by path. `semantic` and `all` are written
/// even when empty, so "no semantic files" is a fingerprint too and compares
/// equal across two revisions that both have none.
pub fn fingerprints(entries: &[SourceEntry], semantic_validated: Option<bool>) -> Vec<Fingerprint> {
    let mut by_kind: BTreeMap<&str, Vec<&SourceEntry>> = BTreeMap::new();
    for entry in entries {
        by_kind.entry(entry.kind).or_default().push(entry);
    }
    let mut out: Vec<Fingerprint> = by_kind
        .iter()
        .map(|(kind, entries)| fingerprint_of(kind, entries, None))
        .collect();
    let semantic: Vec<&SourceEntry> = entries
        .iter()
        .filter(|e| SEMANTIC_KINDS.contains(&e.kind))
        .collect();
    out.push(fingerprint_of("semantic", &semantic, semantic_validated));
    let all: Vec<&SourceEntry> = entries.iter().collect();
    out.push(fingerprint_of("all", &all, None));
    out
}

fn fingerprint_of(kind: &str, entries: &[&SourceEntry], validated: Option<bool>) -> Fingerprint {
    let mut lines: Vec<(&str, &str, &str)> = entries
        .iter()
        .map(|e| (e.file_path.as_str(), e.kind, e.content_sha256.as_str()))
        .collect();
    lines.sort();
    let mut hasher = Sha256::new();
    for (path, kind, hash) in &lines {
        // Length-prefixed, so no two entry lists hash the same by shifting a
        // boundary between path and kind.
        for part in [*path, *kind, *hash] {
            hasher.update((part.len() as u64).to_le_bytes());
            hasher.update(part.as_bytes());
        }
    }
    Fingerprint {
        kind: kind.to_string(),
        fingerprint: hex::encode(hasher.finalize()),
        file_count: lines.len(),
        validated,
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walker::AutomationKind;

    fn entry(path: &str, kind: FileKind, content: &str) -> SourceEntry {
        SourceEntry::new(path, kind, content).expect("a compiled kind")
    }

    #[test]
    fn only_semantic_text_is_kept_and_config_never_is() {
        let view = entry(
            "semantics/views/a.view.yml",
            FileKind::SemanticView,
            "name: a",
        );
        let config = entry(
            "config.yml",
            FileKind::Config,
            "databases: []\npassword: hunter2",
        );
        let agent = entry("a.agentic.yml", FileKind::AgenticAgent, "name: a");
        assert_eq!(view.text.as_deref(), Some("name: a"));
        assert_eq!(config.text, None, "config.yml text would undo redaction");
        assert_eq!(agent.text, None);
        assert_eq!(
            config.content_sha256,
            sha256_hex(b"databases: []\npassword: hunter2")
        );
    }

    #[test]
    fn a_nul_byte_keeps_the_hash_but_not_the_text() {
        let view = entry("v.view.yml", FileKind::SemanticView, "name: a\0");
        assert_eq!(view.text, None);
        assert_eq!(view.content_sha256, sha256_hex(b"name: a\0"));
    }

    #[test]
    fn a_retired_agent_has_no_entry() {
        assert!(SourceEntry::new("x.agent.yml", FileKind::RetiredAgent, "name: x").is_none());
    }

    fn fp<'a>(all: &'a [Fingerprint], kind: &str) -> &'a Fingerprint {
        all.iter()
            .find(|f| f.kind == kind)
            .expect("fingerprint present")
    }

    #[test]
    fn a_change_outside_semantics_leaves_the_semantic_fingerprint_alone() {
        let view = entry(
            "semantics/views/a.view.yml",
            FileKind::SemanticView,
            "name: a",
        );
        let before = vec![
            view.clone(),
            entry(
                "w.automation.yml",
                FileKind::Automation(AutomationKind::Automation),
                "v: 1",
            ),
        ];
        let after = vec![
            view,
            entry(
                "w.automation.yml",
                FileKind::Automation(AutomationKind::Automation),
                "v: 2",
            ),
        ];
        let (b, a) = (fingerprints(&before, None), fingerprints(&after, None));
        assert_eq!(fp(&b, "semantic"), fp(&a, "semantic"));
        assert_ne!(fp(&b, "automation"), fp(&a, "automation"));
        assert_ne!(fp(&b, "all"), fp(&a, "all"));
    }

    #[test]
    fn order_of_discovery_does_not_matter_but_content_and_paths_do() {
        let a = entry(
            "semantics/views/a.view.yml",
            FileKind::SemanticView,
            "name: a",
        );
        let b = entry(
            "semantics/views/b.view.yml",
            FileKind::SemanticView,
            "name: b",
        );
        let forward = fingerprints(&[a.clone(), b.clone()], None);
        let backward = fingerprints(&[b.clone(), a.clone()], None);
        assert_eq!(fp(&forward, "semantic"), fp(&backward, "semantic"));

        let renamed = entry(
            "semantics/views/c.view.yml",
            FileKind::SemanticView,
            "name: a",
        );
        assert_ne!(
            fp(&fingerprints(&[renamed, b], None), "semantic"),
            fp(&forward, "semantic"),
            "a moved file is a different model on the reader's path order"
        );
    }

    #[test]
    fn semantic_and_all_exist_even_when_empty_and_carry_the_verdict() {
        let none = fingerprints(&[], Some(true));
        assert_eq!(fp(&none, "semantic").file_count, 0);
        assert_eq!(fp(&none, "semantic").validated, Some(true));
        assert_eq!(fp(&none, "all").validated, None);
        assert_eq!(
            fp(&none, "semantic"),
            fp(&fingerprints(&[], Some(true)), "semantic")
        );
    }
}
