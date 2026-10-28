//! Prompt constants and shared prompt-formatting helpers for the builder
//! agent.
//!
//! Domain knowledge — the shape of semantic views, topics, and
//! agents — lives in vendored reference documents under
//! `crates/agentic/builder/knowledge/` and is embedded here via
//! [`include_str!`]. Keeping it out of the task-specific prompt strings
//! means we have a single source of truth that survives schema changes
//! and stays in sync with the platform skills in `sdk/cli/skills/`. See
//! `knowledge/README.md` for the sync process.
//!
//! This mirrors `crates/agentic/analytics/src/solver/prompts.rs`, which
//! is the precedent for prompt-constant organization inside an agentic
//! crate.

/// Condensed reference for authoring `.view.yml` and `.topic.yml` files.
pub(crate) const SEMANTIC_LAYER_REFERENCE: &str =
    include_str!("../knowledge/semantic-layer-reference.md");

/// Verbatim template for a semantic view file.
pub(crate) const VIEW_TEMPLATE: &str = include_str!("../knowledge/view-template.yml");

/// Verbatim template for a semantic topic file.
pub(crate) const TOPIC_TEMPLATE: &str = include_str!("../knowledge/topic-template.yml");

/// Reference for authoring `.agentic.yml` files — multi-step FSM agents,
/// per-state overrides, validation rules, semantic engine wiring.
pub(crate) const AGENTIC_BUILDER_REFERENCE: &str =
    include_str!("../knowledge/agentic-builder-reference.md");

/// Verbatim template for an `.agentic.yml` file.
pub(crate) const AGENTIC_TEMPLATE: &str = include_str!("../knowledge/agentic-template.yml");

/// One of the authored reference cards.  Used both as a typed
/// parameter for the per-onboarding-phase pre-population path and as
/// the closed enum the runtime `lookup_reference` tool accepts.
///
/// Variant order is the canonical sort order for cache stability — see
/// [`reference_context`].  Do not reorder without updating the cache
/// regression tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnowledgeCard {
    SemanticLayer,
    AgenticBuilder,
}

impl KnowledgeCard {
    /// Stable lower-kebab-case identifier used by the `lookup_reference`
    /// tool's JSON schema and the run-metadata persistence path.
    pub fn slug(self) -> &'static str {
        match self {
            KnowledgeCard::SemanticLayer => "semantic-layer",
            KnowledgeCard::AgenticBuilder => "agentic-builder",
        }
    }

    /// Parse a slug back into a card.  Used by the runtime tool to
    /// validate the LLM's `card_name` argument.
    pub fn from_slug(s: &str) -> Option<Self> {
        match s {
            "semantic-layer" => Some(KnowledgeCard::SemanticLayer),
            "agentic-builder" => Some(KnowledgeCard::AgenticBuilder),
            _ => None,
        }
    }

    /// Every variant, in canonical order.
    pub fn all() -> [KnowledgeCard; 2] {
        [KnowledgeCard::SemanticLayer, KnowledgeCard::AgenticBuilder]
    }

    /// Section header used when the card is composed into a system
    /// prompt or returned by `lookup_reference`.
    fn section_header(self) -> &'static str {
        match self {
            KnowledgeCard::SemanticLayer => "## Semantic model reference",
            KnowledgeCard::AgenticBuilder => "## Agentic agent reference",
        }
    }

    /// Render the card body — section header, card markdown, plus any
    /// associated verbatim YAML templates — with provenance headers
    /// stripped so they cannot leak into the prompt.
    pub(crate) fn content_with_templates(self) -> String {
        match self {
            KnowledgeCard::SemanticLayer => format!(
                "{header}\n\n{card}\n\n\
                 ### View template (`*.view.yml` — anywhere under `semantics/`)\n\n\
                 ```yaml\n{view}\n```\n\n\
                 ### Topic template (`*.topic.yml` — anywhere under `semantics/`)\n\n\
                 ```yaml\n{topic}\n```",
                header = self.section_header(),
                card = strip_vendoring_header(SEMANTIC_LAYER_REFERENCE),
                view = strip_vendoring_header(VIEW_TEMPLATE),
                topic = strip_vendoring_header(TOPIC_TEMPLATE),
            ),
            KnowledgeCard::AgenticBuilder => format!(
                "{header}\n\n{card}\n\n\
                 ### Agentic template (`*.agentic.yml`)\n\n\
                 ```yaml\n{agentic}\n```",
                header = self.section_header(),
                card = strip_vendoring_header(AGENTIC_BUILDER_REFERENCE),
                agentic = strip_vendoring_header(AGENTIC_TEMPLATE),
            ),
        }
    }
}

/// Always-on index of the available reference cards.  Lists the
/// cards by slug so the model knows which name to pass to
/// `lookup_reference`.  Hand-authored — kept short on purpose.
pub(crate) const CARD_INDEX: &str = "## Reference cards

The following domain reference cards are available.  Call \
`lookup_reference(card_name)` before authoring or modifying a YAML \
file you have not already loaded the schema for in this conversation.

- `semantic-layer` — for `*.view.yml` and `*.topic.yml`.  Entities, \
  dimensions, measures, joins, default filters.
- `agentic-builder` — for `*.agentic.yml`.  Multi-step FSM analytics \
  agent: clarifying / specifying / solving / executing / interpreting.";

/// Always-on cross-cutting rules — invariants that hold regardless of
/// which file type the agent is touching.  Hand-authored.  Keep tight:
/// every line on this list pays for itself across every builder call.
pub(crate) const CROSS_CUTTING_RULES: &str = "## Cross-cutting rules

These hold regardless of which YAML you are touching.  The reference \
card for the file's type may add more rules; if it conflicts with one \
here, the card wins.

- `samples:` is always a list of strings, even when the dimension type \
  is `boolean` or `number`.  Bare booleans/numbers fail YAML \
  deserialization and break workspace load.
- `name:` fields use `snake_case` everywhere — views, topics, \
  dimensions, measures, entities.
- Always read an existing file before editing it; never guess content.
- Use `write_file` for new files or full rewrites, `edit_file` (with \
  exact-match `old_string` / `new_string`) for targeted edits, and \
  `delete_file` to remove a file.  All three suspend for user \
  confirmation, so do not attempt to write the filesystem any other \
  way.
- After a YAML change is accepted, call `validate_project` on the \
  modified file to confirm it is schema-valid.
- Data Apps (`*.app.yml`) are retired.  Never create one, and never \
  offer to: a dashboard or app is a custom app, which engineers \
  build and publish with `oxyc publish`.  Existing `.app.yml` files \
  may be read and, on request, edited.
- A foreign key shown to a person needs a label: declare a `foreign` \
  entity on the FK side and pull the joined view's name dimension, or \
  fall back to `execute_sql` with an explicit JOIN.
- Do not add `# yaml-language-server: $schema=...` directives. \
  Exception: `.agentic.yml` files may carry this directive on line 1 — \
  preserve it if present, but do not add it unless authoring a brand \
  new file from the agentic-builder template.
- When unsure of a sub-type's exact field names, call \
  `lookup_schema(<TypeName>)` for the JSON schema.";

/// Compose a prompt by concatenating a reference card with a
/// task-specific body. Keeps domain knowledge out of the task strings.
pub(crate) fn with_reference(reference: &str, task: &str) -> String {
    format!("## Reference\n\n{reference}\n\n---\n\n{task}")
}

/// Strip provenance headers that sit at the top of vendored knowledge
/// files. Two flavors are recognized:
///
/// 1. YAML frontmatter (`---\n…\n---\n`) — used on authored markdown
///    reference cards to carry machine-readable `source:` and
///    `reconciled-hash:` fields that the drift script reads. It must not
///    leak into the prompt the LLM sees.
/// 2. The `# Vendored from sdk/cli/skills …` comment — prepended by
///    `scripts/sync-skills.sh` to verbatim YAML templates so a reader
///    knows where to edit the source. It must not leak either, because
///    a language model may faithfully reproduce comments in files it
///    generates.
///
/// Both can appear; frontmatter is stripped first, then the vendoring
/// comment, so future authored markdown carrying both is also handled.
fn strip_vendoring_header(s: &str) -> &str {
    // Strip a leading YAML frontmatter block if present.
    let s = if let Some(rest) = s.strip_prefix("---\n") {
        match rest.find("\n---\n") {
            Some(end) => rest[end + 5..].trim_start_matches('\n'),
            None => s,
        }
    } else {
        s
    };

    // Strip the `# Vendored from …` block that `sync-skills.sh` prepends
    // to verbatim YAML templates. Invariant: the sync script emits one
    // or more `#`-prefixed comment lines followed immediately by a
    // single blank line, then the YAML body — never a blank line
    // *within* the header. So `\n\n` reliably marks the header/body
    // boundary. If a future sync-script change ever inserts a blank
    // line between comment lines (e.g. for visual grouping), this
    // finder will truncate too early; the script and this stripper
    // must be updated together.
    if !s.starts_with("# Vendored from") {
        return s;
    }
    match s.find("\n\n") {
        Some(pos) => &s[pos + 2..],
        None => s,
    }
}

/// Compose the cached system-prefix knowledge context.
///
/// Always emits the [`CARD_INDEX`] and [`CROSS_CUTTING_RULES`] blocks —
/// the small always-on summary the builder agent can rely on regardless
/// of phase.  Then appends the full body of each requested card
/// (deduplicated, sorted into canonical order) so the cached prefix is
/// byte-stable across requests that ask for the same set in different
/// orders.
///
/// Pass `&[]` for the interactive builder default: index + rules only,
/// with the agent expected to call `lookup_reference` when it needs
/// schema depth.  Onboarding phases pass the cards they need
/// pre-populated so cache hits land on warm reads.
pub(crate) fn reference_context(cards: &[KnowledgeCard]) -> String {
    let mut canonical: Vec<KnowledgeCard> = cards.to_vec();
    canonical.sort();
    canonical.dedup();

    let mut out = String::new();
    out.push_str(CARD_INDEX);
    out.push_str("\n\n");
    out.push_str(CROSS_CUTTING_RULES);
    for card in canonical {
        out.push_str("\n\n");
        out.push_str(&card.content_with_templates());
    }
    out
}

/// Test-only composer that includes every card.  Kept exclusively for
/// the leak-detection regression test below; production callers go
/// through [`reference_context`] with the cards they need.
#[cfg(test)]
fn full_reference_context() -> String {
    reference_context(&KnowledgeCard::all())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_files_are_embedded_and_non_empty() {
        // Primary guard: include_str! actually resolved a real file.
        assert!(!SEMANTIC_LAYER_REFERENCE.is_empty());
        assert!(!VIEW_TEMPLATE.is_empty());
        assert!(!TOPIC_TEMPLATE.is_empty());
        assert!(!AGENTIC_BUILDER_REFERENCE.is_empty());
        assert!(!AGENTIC_TEMPLATE.is_empty());

        // Structural guard: each template has its schema-defining keys.
        // Prefer stable field names over prose so minor rewrites don't
        // break this test.
        assert!(VIEW_TEMPLATE.contains("entities:"));
        assert!(VIEW_TEMPLATE.contains("dimensions:"));
        assert!(VIEW_TEMPLATE.contains("measures:"));
        assert!(TOPIC_TEMPLATE.contains("base_view:"));
        assert!(AGENTIC_TEMPLATE.contains("instructions:"));
        assert!(AGENTIC_TEMPLATE.contains("states:"));

        // `samples` must be a list of *strings* — bare YAML booleans
        // (`samples: [true, false]`) deserialize as `bool` and crash
        // `SemanticManager::load()` for the entire workspace, since the
        // semantic deserializer expects `Vec<String>`. Guard the template
        // so a future condense pass can't quietly reintroduce the bug.
        assert!(
            VIEW_TEMPLATE.contains(r#"["true", "false"]"#),
            "VIEW_TEMPLATE must use quoted-string boolean samples; \
             see oxy-hq/skills PR #8 for the original incident",
        );
    }

    #[test]
    fn with_reference_composes_sections() {
        let out = with_reference("REF", "TASK");
        assert!(out.contains("## Reference"));
        assert!(out.contains("REF"));
        assert!(out.contains("---"));
        assert!(out.contains("TASK"));
        assert!(out.find("REF").unwrap() < out.find("TASK").unwrap());
    }

    #[test]
    fn strip_vendoring_header_drops_header_when_present() {
        let input = "# Vendored from sdk/cli/skills — do not edit here\n\
                     # Source: sdk/cli/skills/x/templates/view-template.yml\n\
                     \n\
                     name: my_view\n";
        let stripped = strip_vendoring_header(input);
        assert!(!stripped.contains("Vendored from"));
        assert!(stripped.starts_with("name: my_view"));
    }

    #[test]
    fn strip_vendoring_header_is_noop_when_absent() {
        let input = "name: my_view\nexpr: foo\n";
        assert_eq!(strip_vendoring_header(input), input);
    }

    #[test]
    fn strip_vendoring_header_drops_yaml_frontmatter() {
        let input = "---\n\
                     source:\n  - sdk/cli/skills/x/SKILL.md\n\
                     reconciled-hash: deadbeef\n\
                     ---\n\
                     \n\
                     # Title\n\
                     body line\n";
        let stripped = strip_vendoring_header(input);
        assert!(!stripped.contains("source:"));
        assert!(!stripped.contains("reconciled-hash:"));
        assert!(stripped.starts_with("# Title"));
    }

    #[test]
    fn strip_vendoring_header_drops_frontmatter_then_vendoring_comment() {
        let input = "---\n\
                     source:\n  - x.md\n\
                     ---\n\
                     # Vendored from sdk/cli/skills — do not edit here\n\
                     # Source: sdk/cli/skills/x/templates/view-template.yml\n\
                     \n\
                     name: my_view\n";
        let stripped = strip_vendoring_header(input);
        assert!(!stripped.contains("source:"));
        assert!(!stripped.contains("Vendored from"));
        assert!(stripped.starts_with("name: my_view"));
    }

    #[test]
    fn strip_vendoring_header_leaves_unterminated_frontmatter_alone() {
        // A document opening with `---\n` but never closing is not real
        // frontmatter — leave it untouched rather than swallowing it.
        let input = "---\nno closing fence\nbody\n";
        assert_eq!(strip_vendoring_header(input), input);
    }

    #[test]
    fn full_reference_context_strips_template_headers() {
        let out = full_reference_context();
        // The composed output fences YAML templates and embeds markdown
        // reference cards. Neither the vendoring-comment header nor the
        // YAML frontmatter on the cards should leak into the prompt the
        // LLM sees.
        assert!(
            !out.contains("# Vendored from"),
            "vendoring header leaked into full_reference_context: {out}"
        );
        assert!(
            !out.contains("reconciled-hash:"),
            "card frontmatter leaked into full_reference_context: {out}"
        );
        // Every domain area is represented.
        assert!(out.contains("Semantic model reference"));
        assert!(out.contains("View template"));
        assert!(out.contains("Topic template"));
        assert!(!out.contains("Data app reference"));
        assert!(out.contains("Agentic agent reference"));
        assert!(out.contains("Agentic template"));
        // Structural content survives.
        assert!(out.contains("entities:"));
        assert!(out.contains("base_view:"));
        assert!(out.contains("states:"));
    }

    // ── KnowledgeCard / reference_context ───────────────────────────────────

    #[test]
    fn knowledge_card_slugs_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for card in KnowledgeCard::all() {
            let slug = card.slug();
            assert!(seen.insert(slug), "duplicate slug: {slug}");
            assert_eq!(KnowledgeCard::from_slug(slug), Some(card));
        }
        assert_eq!(KnowledgeCard::from_slug("not-a-card"), None);
    }

    #[test]
    fn card_index_lists_all_user_facing_cards() {
        for card in KnowledgeCard::all() {
            assert!(
                CARD_INDEX.contains(card.slug()),
                "CARD_INDEX missing slug {}",
                card.slug()
            );
        }
        // Closed-set guidance: the index must name the runtime tool by
        // hand so the agent knows what to call when it needs a card.
        assert!(CARD_INDEX.contains("lookup_reference"));
    }

    #[test]
    fn cross_cutting_rules_includes_critical_invariants() {
        // Sentinel substrings — the rules covering YAML-load failures,
        // chart-axis bugs, and tool discipline.
        for needle in [
            "samples",
            "snake_case",
            "write_file",
            "edit_file",
            "delete_file",
            "validate_project",
            ".app.yml",
            "yaml-language-server",
        ] {
            assert!(
                CROSS_CUTTING_RULES.contains(needle),
                "CROSS_CUTTING_RULES missing sentinel: {needle}"
            );
        }
    }

    #[test]
    fn reference_context_index_only_is_compact() {
        // Default for the interactive builder: just index + rules, no
        // full cards.  This is the whole point of the trim — keep this
        // tight so cache footprint actually drops.
        let out = reference_context(&[]);
        assert!(
            out.len() < 8000,
            "index-only reference_context grew past 8000 chars: {} chars",
            out.len()
        );
        assert!(out.contains("## Reference cards"));
        assert!(out.contains("## Cross-cutting rules"));
        // None of the full cards should bleed in.
        assert!(!out.contains("entities:"));
        assert!(!out.contains("base_view:"));
    }

    #[test]
    fn reference_context_with_semantic_layer_includes_card_content() {
        let out = reference_context(&[KnowledgeCard::SemanticLayer]);
        assert!(out.contains("## Semantic model reference"));
        // View + topic templates are inlined alongside the card.
        assert!(out.contains("entities:"));
        assert!(out.contains("base_view:"));
        // Other cards are NOT included.
        assert!(!out.contains("## Agentic agent reference"));
    }

    #[test]
    fn reference_context_card_order_is_stable() {
        // Cache-stability invariant: the cached system prefix must not
        // depend on the order callers happen to pass cards in.
        let a = reference_context(&[KnowledgeCard::AgenticBuilder, KnowledgeCard::SemanticLayer]);
        let b = reference_context(&[KnowledgeCard::SemanticLayer, KnowledgeCard::AgenticBuilder]);
        assert_eq!(a, b, "reference_context must sort cards canonically");
    }

    #[test]
    fn reference_context_deduplicates() {
        let a = reference_context(&[KnowledgeCard::SemanticLayer, KnowledgeCard::SemanticLayer]);
        let b = reference_context(&[KnowledgeCard::SemanticLayer]);
        assert_eq!(a, b, "reference_context must deduplicate cards");
    }

    #[test]
    fn reference_context_strips_provenance_headers() {
        // Frontmatter and vendoring comments must not leak for any
        // card combination.  Future hand-authored cards that forget to
        // call strip_vendoring_header would regress this.
        for cards in [
            vec![],
            vec![KnowledgeCard::SemanticLayer],
            vec![KnowledgeCard::AgenticBuilder],
            KnowledgeCard::all().to_vec(),
        ] {
            let out = reference_context(&cards);
            assert!(
                !out.contains("# Vendored from"),
                "vendoring comment leaked for cards={cards:?}"
            );
            assert!(
                !out.contains("reconciled-hash:"),
                "frontmatter leaked for cards={cards:?}"
            );
        }
    }
}
