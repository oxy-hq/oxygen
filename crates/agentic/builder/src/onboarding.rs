//! Onboarding prompt builder — constructs the builder agent prompt from
//! structured onboarding context (tables, warehouse type, model config).
//!
//! Four focused prompts are generated, one per build phase:
//! - `SemanticLayer` — inspect schemas, update config.yml, create .view.yml files (legacy, all-in-one)
//! - `Config`        — update config.yml only (model entry + database defaults)
//! - `SemanticView`  — inspect one table, create matching .view.yml + .topic.yml files
//! - `Agent`         — create analytics.agentic.yml (agentic analytics agent)
//!
//! Onboarding no longer builds starter dashboards: Data Apps (`.app.yml`) are retired
//! in favour of custom apps, so there is no `App` / `App2` phase.
//!
//! This keeps the prompt templates server-side so the frontend only sends
//! structured selections, not raw LLM instructions.

use serde::Deserialize;

use crate::prompts::KnowledgeCard;

/// Fallback model name used when the frontend doesn't supply a `model_config`.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// Which build phase this run covers.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingBuildStep {
    /// Inspect tables, update config.yml, create .view.yml files (legacy all-in-one).
    ///
    /// **Superseded** by the `Config` + `SemanticView` decomposition. Kept only as
    /// a backward-compat fallback for older frontend builds and as the `Default`
    /// target for deserializing requests with no explicit `step`. New callers must
    /// not construct this variant directly.
    #[default]
    SemanticLayer,
    /// Update config.yml only (model entry + database defaults).
    Config,
    /// Inspect one table and create a single .view.yml file.
    /// Uses the first entry in `tables` as the target table.
    SemanticView,
    /// Create the default agentic analytics agent (`analytics.agentic.yml`).
    ///
    /// This replaces the legacy `.agent.yml` classic-agent template — users
    /// onboarding now end up with the multi-step FSM analytics pipeline
    /// (`agentic-analytics`) rather than a single-turn tool-calling agent.
    Agent,
}

/// A column definition pre-fetched during schema discovery.
#[derive(Debug, Clone, Deserialize)]
pub struct TableColumnDef {
    pub name: String,
    #[serde(alias = "type")]
    pub column_type: String,
}

/// Structured context sent from the onboarding frontend.
#[derive(Debug, Clone, Deserialize)]
pub struct OnboardingContext {
    /// Selected tables in "schema.table" format.
    pub tables: Vec<String>,
    /// Warehouse type (e.g., "clickhouse", "postgres", "bigquery").
    pub warehouse_type: String,
    #[serde(default)]
    pub model_config: Option<OnboardingModelConfig>,
    /// Which build phase this run is for.
    #[serde(default)]
    pub step: OnboardingBuildStep,
    /// Pre-fetched column definitions for the target table (avoids DESCRIBE round-trip).
    #[serde(default)]
    pub table_schema: Option<Vec<TableColumnDef>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OnboardingModelConfig {
    pub name: String,
    pub vendor: String,
    pub model_ref: String,
    pub key_var: String,
}

impl OnboardingContext {
    /// Build the focused prompt for the requested build phase.
    pub fn build_prompt(&self) -> String {
        match self.step {
            OnboardingBuildStep::SemanticLayer => self.build_semantic_layer_prompt(),
            OnboardingBuildStep::Config => self.build_config_prompt(),
            OnboardingBuildStep::SemanticView => self.build_semantic_view_prompt(),
            OnboardingBuildStep::Agent => self.build_agent_prompt(),
        }
    }

    /// Reference cards to pre-populate into the builder solver's
    /// cached system prefix for this phase.  Each phase pulls in only
    /// the cards relevant to the artifact it produces, keeping the
    /// per-phase cache entry tight.  Interactive (non-onboarding)
    /// builder runs use no cards by default and rely on the
    /// `lookup_reference` tool.
    pub fn knowledge_cards(&self) -> Vec<KnowledgeCard> {
        use KnowledgeCard::*;
        match self.step {
            // config.yml has no opinionated card; the builder writes
            // the model entry and database default from inline guidance.
            OnboardingBuildStep::Config => vec![],
            // .view.yml + .topic.yml — both covered by the semantic-layer card.
            OnboardingBuildStep::SemanticView | OnboardingBuildStep::SemanticLayer => {
                vec![SemanticLayer]
            }
            // analytics.agentic.yml — covered by the agentic-builder card.
            OnboardingBuildStep::Agent => vec![AgenticBuilder],
        }
    }

    /// Tools to expose to the builder for this onboarding phase.
    /// Drops the 15+ dbt/airform tools, `search_text`, `run_tests`,
    /// and `manage_directory` from every phase, and trims warehouse
    /// tools (`execute_sql`, `semantic_query`) out of phases that
    /// don't need them.  Reduces tool-selection noise and shrinks the
    /// cached system prefix.
    pub fn tool_allowlist(&self) -> Vec<String> {
        // Common across every onboarding phase: file authoring, schema
        // reference, validation, and the HITL escape hatch.
        let common: &[&str] = &[
            "search_files",
            "read_file",
            "write_file",
            "edit_file",
            "delete_file",
            "validate_project",
            "lookup_reference",
            "lookup_schema",
            "ask_user",
        ];
        // Phases that touch the warehouse (DESCRIBE TABLE, smoke-test
        // queries, data profiling) need execute_sql + semantic_query.
        let warehouse: &[&str] = &["execute_sql", "semantic_query"];

        let extras: &[&str] = match self.step {
            OnboardingBuildStep::Config | OnboardingBuildStep::Agent => &[],
            OnboardingBuildStep::SemanticView | OnboardingBuildStep::SemanticLayer => warehouse,
        };

        common
            .iter()
            .chain(extras.iter())
            .map(|s| s.to_string())
            .collect()
    }

    // ── helpers ──────────────────────────────────────────────────────────────

    fn table_list(&self) -> String {
        self.tables
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn model_name(&self) -> &str {
        self.model_config
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or(DEFAULT_MODEL)
    }

    // ── Phase 1a: Config only ────────────────────────────────────────────────

    fn build_config_prompt(&self) -> String {
        let db_name = &self.warehouse_type;

        let model_instructions = if let Some(mc) = &self.model_config {
            format!(
                r#"
- A model entry (if not already present):
```yaml
models:
  - name: {name}
    vendor: {vendor}
    model_ref: {model_ref}
    key_var: {key_var}
```
- Builder agent config: `builder_agent: {{ model: {name} }}`
"#,
                name = mc.name,
                vendor = mc.vendor,
                model_ref = mc.model_ref,
                key_var = mc.key_var,
            )
        } else {
            String::new()
        };

        format!(
            r#"I just connected a {db_name} warehouse.

Your task: **update config.yml** with the required configuration.

Use `edit_file` for targeted updates to existing keys, or `write_file` if config.yml does not yet exist (or you must replace the entire file).

---

## Instructions

Read the existing config.yml first. Then propose changes to ensure it has:
{model_instructions}
- A `defaults.database` pointing to `{db_name}` if not set

Do NOT create any other files. Only update config.yml.

Prefer `edit_file` with a precise `old_string` / `new_string` pair for each missing or stale block — that avoids touching unrelated content. Only fall back to `write_file` if config.yml is missing or you genuinely need to replace the entire file. Whichever you choose, perform **exactly one** write call for config.yml — no revisions, no re-drafts.

After proposing the change, STOP — do NOT write a summary or explanation."#,
        )
    }

    // ── Phase 1b: Single semantic view ───────────────────────────────────────

    fn build_semantic_view_prompt(&self) -> String {
        let db_name = &self.warehouse_type;
        // Use the first table in the list as the target
        let table = self
            .tables
            .first()
            .map(|t| t.as_str())
            .unwrap_or("unknown_table");

        let view_name = table.rsplit('.').next().unwrap_or(table);

        // Cross-table FK awareness: list every other selected table so the
        // agent can declare `type: foreign` entities pointing at views that
        // will exist in the same workspace. Without this the agent has no
        // way to know which `*_id` columns are real FKs vs opaque strings,
        // and onboarding-generated topics end up rendering raw UUIDs on
        // chart axes (no labeled join possible).
        let has_other_tables = self.tables.len() > 1;
        let other_tables_section = if has_other_tables {
            let others = self
                .tables
                .iter()
                .filter(|t| t.as_str() != table)
                .map(|t| {
                    let other_view = t.rsplit('.').next().unwrap_or(t.as_str());
                    format!("  - `{t}` (view name will be `{other_view}`)")
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                r#"

## Cross-table FK awareness

The user also selected these other tables in this workspace; matching `.view.yml` files will exist alongside the one you create now:
{others}

If this view has a column that is a foreign key into one of those tables (typical patterns: `<other>_id`, `<other>_guid`, or a column that obviously names another table like `restaurant_id` → restaurants), you MUST declare a `type: foreign` entity for it. The foreign entity's `name:` must match what the lookup view's primary entity will be named (use the lookup view's table name without prefixes — `restaurant`, `customer`, `order` — singular). The foreign entity's `key:` must reference the FK dimension on THIS view.

```yaml
entities:
  - name: <this_view_subject>
    type: primary
    key: <pk_dim>
  - name: <lookup_view_subject>      # e.g. `restaurant` if the lookup table is restaurants
    type: foreign
    key: <fk_dim_on_this_view>       # e.g. `restaurant_id`
```

Without this declaration the analytics agent has no way to surface the lookup view's name column next to a measure and falls back to returning raw UUIDs. The semantic-layer reference card has the full rule under "Critical rules" #5."#
            )
        } else {
            String::new()
        };

        let foreign_entity_bullet = if has_other_tables {
            "\n- Plus a `type: foreign` entity for every FK column pointing at one of the other selected tables (see \"Cross-table FK awareness\" above)."
        } else {
            ""
        };

        let topic_fk_views_comment = if has_other_tables {
            r#"
  # If you declared any `type: foreign` entities on the view above, also list
  # those lookup views here. Example: a fact view that declares a foreign
  # `restaurant` entity should include `restaurants` in this list, so the
  # analytics agent can pull labels (e.g. `restaurants.location_name`)
  # via semantic_query instead of rendering raw FK UUIDs."#
        } else {
            ""
        };

        let topic_fk_motivation = if has_other_tables {
            " **Including FK-target views in `views:` is what unlocks human-readable labels in answers** — without it the analytics agent has to either return UUIDs or fall back to raw SQL JOINs."
        } else {
            ""
        };

        // If pre-fetched schema is available, inline it and skip the DESCRIBE step.
        let (schema_section, view_step) = match &self.table_schema {
            Some(cols) if !cols.is_empty() => {
                let col_list = cols
                    .iter()
                    .map(|c| format!("  - `{}` ({})", c.name, c.column_type))
                    .collect::<Vec<_>>()
                    .join("\n");
                (
                    format!(
                        r#"## Table schema (pre-fetched)

Table: `{table}`
Columns:
{col_list}

Use these columns directly — do NOT run any SQL queries."#
                    ),
                    1, // Create the view file at step 1
                )
            }
            _ => (
                format!(
                    r#"## Step 1: Inspect the table schema

Use execute_sql with `DESCRIBE TABLE {table}` (or the {db_name}-equivalent).
Understand column names, types, and cardinality."#
                ),
                2, // Create the view file at step 2
            ),
        };

        let topic_step = view_step + 1;
        let smoke_test_step = topic_step + 1;

        format!(
            r#"I need a semantic model entry for a single table in my {db_name} warehouse.

Your task: **create two files** — a `.view.yml` and a matching `.topic.yml` — for table `{table}` — and smoke-test the result.

Use `write_file` for each new file (these are brand new — no existing content to preserve).

---

{schema_section}{other_tables_section}

## Step {view_step}: Create the view file

Create `semantics/{view_name}.view.yml`. Use:

- `name: {view_name}`
- `datasource: {db_name}`
- `table: "{table}"`
- One primary entity, plus 3–8 dimensions and 2–4 measures that make analytical sense for this table.{foreign_entity_bullet}

The full schema (entity rules, allowed dimension/measure types, `expr` requirements, naming conventions, **and the per-warehouse date-column recipes**) is in the `## Semantic model reference` section of your system prompt — follow it exactly. Pick a primary-key dimension (id, uuid, or the most specific unique column).

**Date columns are the most common foot-gun on {db_name}.** Any column whose business meaning is a date or timestamp (`*_date`, `*_at`, `business_date`, `created`, `updated`, `event_time`) MUST be declared `type: date` (or `type: datetime`) and wrapped via the per-warehouse recipe in the reference card — not `type: number` or `type: string` over the raw column. Mismatch produces a silent `TYPE_MISMATCH` the first time the analytics agent filters on the dimension.

## Step {topic_step}: Create the topic file

Create `semantics/{view_name}.topic.yml`:

```yaml
name: {view_name}
description: "<one-line description of the business domain this topic covers>"
base_view: {view_name}
views:
  - {view_name}{topic_fk_views_comment}
```

Topics are what the analytics agent queries against — every view needs a matching topic.{topic_fk_motivation}

Call `write_file` **exactly once** per file (once for the view, once for the topic). Pass the full file contents in the `content` argument. Do not call `write_file` again for the same file — no revisions, no re-drafts.

## Step {smoke_test_step}: Smoke-test the view

Before declaring victory, prove the view actually works end-to-end by calling `semantic_query` against the topic you just created. The most failure-prone path is filtering on a date dimension, so target that:

1. Pick the primary date/datetime dimension on the view (if there is one) and the most business-interesting measure.
2. Call `semantic_query(topic="{view_name}", dimensions=["{view_name}.<date_dim>"], measures=["{view_name}.<primary_measure>"], limit=5)`.  No filter is needed — the goal is to prove the topic compiles and the dimension/measure expressions execute.
3. If the view has no date-like dimension, run `semantic_query(topic="{view_name}", measures=["{view_name}.<primary_measure>"], limit=1)` instead.

If the smoke test **succeeds**: stop, the phase is done.

If the smoke test **fails** (`TYPE_MISMATCH`, compile error, "column not found", etc.): diagnose the error, apply a single corrective `edit_file` to the view file (precise `old_string` / `new_string`), and re-run `semantic_query` once. If the second attempt still fails, stop and report the error — do NOT keep iterating.

After the smoke test passes (or you've stopped after one fix attempt), STOP — do NOT write a summary, explanation, or any follow-up text."#,
        )
    }

    // ── Phase 1 (legacy): Semantic Model ─────────────────────────────────────

    fn build_semantic_layer_prompt(&self) -> String {
        let table_list = self.table_list();
        let db_name = &self.warehouse_type;

        let model_instructions = if let Some(mc) = &self.model_config {
            format!(
                r#"
- A model entry (if not already present):
```yaml
models:
  - name: {name}
    vendor: {vendor}
    model_ref: {model_ref}
    key_var: {key_var}
```
- Builder agent config: `builder_agent: {{ model: {name} }}`
"#,
                name = mc.name,
                vendor = mc.vendor,
                model_ref = mc.model_ref,
                key_var = mc.key_var,
            )
        } else {
            String::new()
        };

        format!(
            r#"I just connected a {db_name} warehouse and selected the following tables for my semantic model:

{table_list}

Your task for this step: **inspect the tables and create the semantic model**.

Use `write_file` for new files (the `.view.yml` / `.topic.yml` you author) and `edit_file` for targeted updates to existing files like `config.yml`.

---

## Step 1: Inspect table schemas

Use execute_sql with `DESCRIBE TABLE <table>` (or the {db_name}-equivalent) for each selected table.
Understand column names, types, and cardinality before creating any files.

## Step 2: Update config.yml

Read the existing config.yml first. Then propose changes to ensure it has:
{model_instructions}
- A `defaults.database` pointing to `{db_name}` if not set

## Step 3: Create semantic model views

For each table, create a `.view.yml` file under `semantics/<view_name>.view.yml` and a matching `semantics/<view_name>.topic.yml` so the analytics agent can query it.

Use:

- `datasource: {db_name}`
- `table: "<fully_qualified_table_name>"` (matching one of the selected tables above)
- One primary entity, plus dimensions and measures that make analytical sense for the table's data.

The full schema (entity rules, allowed dimension/measure types, `expr` requirements, naming conventions, the matching topic shape, **and the per-warehouse date-column recipes**) is in the `## Semantic model reference` section of your system prompt — follow it exactly. Pay particular attention to the date-column recipes: any `*_date`/`*_at` column on {db_name} that isn't already a Date type needs a wrapping cast, otherwise filters will fail at query time with `TYPE_MISMATCH`.

## Step 4: Smoke-test each view

After all view+topic pairs are written, prove each one actually works by calling `semantic_query` against the topic with its primary date dimension (if any) and primary measure: `semantic_query(topic=<topic_name>, dimensions=[<date_dim>], measures=[<primary_measure>], limit=5)`.  If a topic has no date-like dimension, just query the measure: `semantic_query(topic=<topic_name>, measures=[<primary_measure>], limit=1)`.

If any smoke test fails (`TYPE_MISMATCH`, compile error, "column not found"): diagnose, fix the view via a single `edit_file` (precise `old_string` / `new_string`), re-run the failing smoke test once. If the retry still fails, stop and report the error — do NOT keep iterating.

After all smoke tests pass (or you've stopped after one fix attempt per view), STOP — do NOT write a summary or explanation."#,
        )
    }

    // ── Phase 2: Analytics Agent ──────────────────────────────────────────────

    fn build_agent_prompt(&self) -> String {
        let db_name = &self.warehouse_type;
        let model_name = self.model_name();
        let table_list = self.table_list();

        format!(
            r#"The semantic model for the {db_name} warehouse has just been created — views (`semantics/*.view.yml`) and matching topics (`semantics/*.topic.yml`).

Your task for this step: **create the default agentic analytics agent**.

Use `write_file` ONCE to create `analytics.agentic.yml`.

---

The view + topic files were just created in the previous onboarding step, so you do NOT need to read them or verify they exist — go straight to creating the agentic file.  The `context:` glob in the template below wires the whole `semantics/` tree into the pipeline automatically; you do NOT need to enumerate topic names into the agent file.

## Create analytics.agentic.yml

Call `write_file` exactly once, targeting `analytics.agentic.yml` at the project root. The `content` argument must be the file body below **verbatim** — do not duplicate it, do not wrap it in another document, do not append a second copy.

This is the agentic analytics agent users will interact with to ask questions about their data. It runs a multi-step FSM pipeline (clarify → specify → generate SQL → execute → interpret) rather than a single LLM tool loop. See the `## Agentic agent reference` section in your system prompt for the full schema, per-state overrides, validation rules, and common errors.

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/agentic.json
llm:
  ref: {model_name}

databases:
  - {db_name}

context:
  - ./semantics/**/*
  - ./example_sql/**/*.sql

states:
  specifying:
    max_retries: 10
  interpreting:
    thinking: disabled
```

Constraints:
- First line MUST be the `# yaml-language-server: $schema=…` directive shown above.
- Exactly one YAML document; each top-level key appears at most once.
- Do NOT create an `.agent.yml` file — the legacy classic-agent format is no longer used for onboarding.

After the single `write_file` call, do NOT call `write_file` again for this file. Once it is accepted, output **only** a "Sample Questions" section with 5 numbered questions, each ending in a question mark, that a user could ask this analytics agent about these tables. Nothing else — no summary, no explanation.

{table_list}"#,
        )
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(step: OnboardingBuildStep) -> OnboardingContext {
        OnboardingContext {
            tables: vec!["public.orders".to_string(), "public.customers".to_string()],
            warehouse_type: "postgres".to_string(),
            model_config: Some(OnboardingModelConfig {
                name: "sonnet-4-6".to_string(),
                vendor: "anthropic".to_string(),
                model_ref: "claude-sonnet-4-6".to_string(),
                key_var: "ANTHROPIC_API_KEY".to_string(),
            }),
            step,
            table_schema: None,
        }
    }

    /// Extract the first ```yaml fenced block from a prompt. Panics if none exists.
    fn extract_yaml_block(prompt: &str) -> &str {
        let start = prompt
            .find("```yaml\n")
            .expect("prompt has no ```yaml fence");
        let body_start = start + "```yaml\n".len();
        let end_offset = prompt[body_start..]
            .find("\n```")
            .expect("yaml fence is not closed");
        &prompt[body_start..body_start + end_offset]
    }

    // ── Agent phase: new .agentic.yml template ──────────────────────────────

    #[test]
    fn agent_prompt_creates_agentic_yml_at_project_root() {
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(
            prompt.contains("analytics.agentic.yml"),
            "expected prompt to reference analytics.agentic.yml, got: {prompt}"
        );
        // Must not instruct the builder to create a classic .agent.yml.
        assert!(
            !prompt.contains("agents/default.agent.yml"),
            "prompt should no longer reference the legacy agents/default.agent.yml path"
        );
    }

    #[test]
    fn agent_prompt_mentions_warehouse_and_model() {
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(
            prompt.contains("postgres"),
            "prompt should reference the configured warehouse name"
        );
        assert!(
            prompt.contains("sonnet-4-6"),
            "prompt should reference the configured model name (llm.ref)"
        );
    }

    #[test]
    fn agent_prompt_falls_back_to_default_model() {
        let mut ctx = ctx_with(OnboardingBuildStep::Agent);
        ctx.model_config = None;
        let prompt = ctx.build_prompt();
        assert!(
            prompt.contains(DEFAULT_MODEL),
            "prompt should fall back to DEFAULT_MODEL when no model_config is supplied"
        );
    }

    #[test]
    fn agent_prompt_yaml_block_parses() {
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        let yaml = extract_yaml_block(&prompt);
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)
            .unwrap_or_else(|e| panic!("embedded YAML is not valid YAML: {e}\n---\n{yaml}"));

        // Top-level shape matches agentic-analytics AgentConfig.
        let map = value.as_mapping().expect("top-level YAML must be a map");
        let llm = map
            .get(serde_yaml::Value::String("llm".into()))
            .expect("missing llm: section");
        let llm_map = llm.as_mapping().expect("llm: must be a map");
        let llm_ref = llm_map
            .get(serde_yaml::Value::String("ref".into()))
            .and_then(|v| v.as_str())
            .expect("missing llm.ref field");
        assert_eq!(llm_ref, "sonnet-4-6");

        let databases = map
            .get(serde_yaml::Value::String("databases".into()))
            .and_then(|v| v.as_sequence())
            .expect("missing databases: sequence");
        let db_names: Vec<&str> = databases.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(db_names, vec!["postgres"]);

        let context = map
            .get(serde_yaml::Value::String("context".into()))
            .and_then(|v| v.as_sequence())
            .expect("missing context: sequence");
        let globs: Vec<&str> = context.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            globs.iter().any(|g| g.contains("semantics")),
            "context must include a semantics/** glob, got: {globs:?}"
        );

        // states overrides are present and parse.
        let states = map
            .get(serde_yaml::Value::String("states".into()))
            .and_then(|v| v.as_mapping())
            .expect("missing states: map");
        assert!(
            states.contains_key(serde_yaml::Value::String("specifying".into())),
            "expected a `specifying` state override in states:"
        );
    }

    #[test]
    fn agent_prompt_tells_builder_to_use_write_file() {
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(
            prompt.contains("write_file"),
            "prompt should instruct the builder to use the write_file tool"
        );
    }

    #[test]
    fn agent_prompt_embeds_yaml_language_server_schema_directive() {
        // The embedded template must carry the IDE schema directive so the
        // rendered file is validated against `json-schemas/agentic.json`.
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        let yaml = extract_yaml_block(&prompt);
        assert!(
            yaml.lines().next().is_some_and(|first| first
                .starts_with("# yaml-language-server: $schema=")
                && first.contains("agentic.json")),
            "first line of embedded YAML must be the yaml-language-server directive pointing at agentic.json, got:\n{yaml}"
        );
    }

    #[test]
    fn agent_prompt_forbids_duplicate_writes() {
        // Regression guard against the LLM emitting the file content twice
        // (duplicate top-level keys → invalid YAML → 400 from the backend).
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(
            prompt.contains("exactly once") || prompt.contains("verbatim"),
            "prompt should explicitly forbid duplicate writes; got:\n{prompt}"
        );
    }

    #[test]
    fn build_step_dispatches_agent_variant_to_agent_prompt() {
        // Sanity check: the Agent enum variant actually dispatches through
        // build_agent_prompt (and not a stale variant from an earlier rename).
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(prompt.contains("agentic analytics agent"));
    }

    #[test]
    fn other_phases_do_not_reference_legacy_agent_path() {
        // Regression guard: config / semantic_view prompts should
        // no longer instruct the builder to produce a classic .agent.yml file.
        for step in [
            OnboardingBuildStep::Config,
            OnboardingBuildStep::SemanticView,
        ] {
            let prompt = ctx_with(step.clone()).build_prompt();
            assert!(
                !prompt.contains("agents/default.agent.yml"),
                "phase {step:?} still mentions legacy agents/default.agent.yml"
            );
        }
    }

    // ── SemanticView phase: view + topic schema guards ──────────────────────

    #[test]
    fn semantic_view_prompt_points_at_reference_card() {
        // The full view/topic schema is now carried in the cached
        // semantic-layer reference card (see `crate::prompts::full_reference_context`
        // and the `knowledge_files_are_embedded_and_non_empty` test in
        // prompts.rs which pins entity / dimension / measure presence).
        // The user-facing prompt only needs to nudge the agent at it.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains("Semantic model reference"),
            "SemanticView prompt must defer to the cached semantic-layer reference card; got:\n{prompt}"
        );
    }

    #[test]
    fn semantic_view_prompt_creates_topic_file() {
        // Every view must ship with a matching .topic.yml — the analytics
        // agent's semantic_query tool queries against topics.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains(".topic.yml"),
            "SemanticView prompt must instruct the agent to create a .topic.yml file"
        );
        assert!(
            prompt.contains("base_view:"),
            "SemanticView topic template must include a base_view: field"
        );
    }

    #[test]
    fn semantic_view_prompt_lists_other_selected_tables() {
        // When the user selected multiple tables, each per-table SemanticView
        // run must show the agent the FULL list so it can declare foreign
        // entities for FK columns pointing at those other tables. Without
        // this, FK-shape declarations are inconsistent across the views and
        // the analytics agent ends up returning raw UUIDs.
        let ctx = ctx_with(OnboardingBuildStep::SemanticView);
        let prompt = ctx.build_prompt();
        // `ctx_with` seeds two tables: "public.orders" and "public.customers".
        // The first is the target; the prompt must explicitly reference the
        // second under the cross-table awareness section.
        assert!(
            prompt.contains("Cross-table FK awareness"),
            "SemanticView prompt must include a Cross-table FK awareness section when >1 table is selected"
        );
        assert!(
            prompt.contains("public.customers"),
            "SemanticView prompt must list the other selected tables (e.g. `public.customers`); got:\n{prompt}"
        );
        // Must NOT echo the target table inside the "other tables" list — the
        // target is the one being built, not a sibling reference.
        let cross_section_start = prompt
            .find("Cross-table FK awareness")
            .expect("section guarded above");
        let section_tail = &prompt[cross_section_start..];
        // Cheap proxy: the bullet line for the target table starts with
        // "  - `public.orders`". It must not appear as a SIBLING (it does
        // appear elsewhere in the prompt as the build target).
        assert!(
            !section_tail.contains("- `public.orders`"),
            "Cross-table FK awareness section must not list the target table as a sibling; got:\n{section_tail}"
        );
    }

    #[test]
    fn semantic_view_prompt_omits_cross_table_section_when_single_table() {
        // The cross-table FK section adds prompt tokens; it should be
        // suppressed when only one table is selected to avoid wasted context.
        let mut ctx = ctx_with(OnboardingBuildStep::SemanticView);
        ctx.tables = vec!["public.orders".to_string()];
        let prompt = ctx.build_prompt();
        assert!(
            !prompt.contains("Cross-table FK awareness"),
            "SemanticView prompt must skip the Cross-table FK awareness section when only one table is selected"
        );
    }

    #[test]
    fn semantic_view_prompt_instructs_foreign_entity_declaration() {
        // The cross-table section must explicitly tell the agent to declare
        // a `type: foreign` entity for FK columns and explain why (so it
        // unlocks labeled axes downstream). Round-3 of #2206 added this rule
        // to the cards but the SemanticView phase wasn't surfacing it on
        // every run; without explicit per-prompt mention the rule fires
        // inconsistently.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains("type: foreign"),
            "SemanticView prompt must explicitly mention `type: foreign` entities for FK columns"
        );
        assert!(
            prompt.contains("foreign key") || prompt.contains("FK"),
            "SemanticView prompt must explain that the foreign-entity rule applies to FK columns"
        );
        // The rule must motivate WHY — without the why the agent treats it
        // as boilerplate and skips it on long views.
        assert!(
            prompt.contains("UUID") || prompt.contains("label"),
            "SemanticView prompt must motivate the foreign-entity rule by referencing UUID/label downstream impact"
        );
    }

    #[test]
    fn semantic_view_topic_template_documents_fk_target_views() {
        // Topic files only ever including `views: [<single view>]` is the
        // SECOND structural reason FK→label resolution fails — even when
        // foreign entities are declared, semantic_query can't reach the
        // lookup view's name dimension if the topic doesn't include it.
        // The topic template must explicitly tell the agent to add FK-target
        // views to the topic's `views:` list.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains("FK-target views")
                || prompt.contains("lookup views here")
                || prompt.contains("lookup view"),
            "SemanticView topic template must instruct adding FK-target views to the topic's views: list"
        );
    }

    // ── Legacy SemanticLayer phase: keep it aligned with new schema ─────────

    // ── knowledge_cards() per phase ─────────────────────────────────────────

    #[test]
    fn onboarding_context_knowledge_cards_per_phase() {
        use KnowledgeCard::*;
        let cases: &[(OnboardingBuildStep, &[KnowledgeCard])] = &[
            (OnboardingBuildStep::Config, &[]),
            (OnboardingBuildStep::SemanticView, &[SemanticLayer]),
            (OnboardingBuildStep::SemanticLayer, &[SemanticLayer]),
            (OnboardingBuildStep::Agent, &[AgenticBuilder]),
        ];
        for (step, expected) in cases {
            let cards = ctx_with(step.clone()).knowledge_cards();
            assert_eq!(
                cards.as_slice(),
                *expected,
                "phase {step:?} returned wrong knowledge_cards"
            );
        }
    }

    #[test]
    fn onboarding_tool_allowlist_drops_dbt_and_irrelevant_tools() {
        // Every phase MUST exclude the 15+ dbt/airform tools, search_text,
        // run_tests, and manage_directory.  These belong to the chat
        // builder's full kit and only confuse the onboarding model.
        let banned = [
            "search_text",
            "run_tests",
            "manage_directory",
            "list_dbt_projects",
            "list_dbt_nodes",
            "compile_dbt_model",
            "run_dbt_models",
            "test_dbt_models",
            "get_dbt_lineage",
            "analyze_dbt_project",
            "get_dbt_column_lineage",
            "parse_dbt_project",
            "seed_dbt_project",
            "debug_dbt_project",
            "clean_dbt_project",
            "docs_generate_dbt",
            "format_dbt_sql",
            "init_dbt_project",
        ];
        for step in [
            OnboardingBuildStep::Config,
            OnboardingBuildStep::SemanticView,
            OnboardingBuildStep::SemanticLayer,
            OnboardingBuildStep::Agent,
        ] {
            let allowlist = ctx_with(step.clone()).tool_allowlist();
            for bad in &banned {
                assert!(
                    !allowlist.iter().any(|t| t == bad),
                    "phase {step:?} surfaces banned tool {bad}"
                );
            }
        }
    }

    #[test]
    fn onboarding_tool_allowlist_includes_common_tools_every_phase() {
        // The minimal common set: file authoring, schema/reference
        // lookup, validation, HITL escape hatch.  Every phase needs
        // these regardless of artifact type.
        let common = [
            "search_files",
            "read_file",
            "write_file",
            "edit_file",
            "delete_file",
            "validate_project",
            "lookup_reference",
            "lookup_schema",
            "ask_user",
        ];
        for step in [
            OnboardingBuildStep::Config,
            OnboardingBuildStep::SemanticView,
            OnboardingBuildStep::SemanticLayer,
            OnboardingBuildStep::Agent,
        ] {
            let allowlist = ctx_with(step.clone()).tool_allowlist();
            for needed in &common {
                assert!(
                    allowlist.iter().any(|t| t == needed),
                    "phase {step:?} missing required tool {needed}"
                );
            }
        }
    }

    #[test]
    fn onboarding_tool_allowlist_warehouse_phases_get_sql_and_semantic_query() {
        // Phases that touch the warehouse need execute_sql (DESCRIBE,
        // smoke test, profiling) and semantic_query (smoke test,
        // verification).  Phases that don't touch the warehouse must
        // NOT be exposed to those tools.
        let warehouse_phases = [
            OnboardingBuildStep::SemanticView,
            OnboardingBuildStep::SemanticLayer,
        ];
        let non_warehouse_phases = [OnboardingBuildStep::Config, OnboardingBuildStep::Agent];

        for step in warehouse_phases {
            let allowlist = ctx_with(step.clone()).tool_allowlist();
            assert!(allowlist.iter().any(|t| t == "execute_sql"));
            assert!(allowlist.iter().any(|t| t == "semantic_query"));
        }
        for step in non_warehouse_phases {
            let allowlist = ctx_with(step.clone()).tool_allowlist();
            assert!(!allowlist.iter().any(|t| t == "execute_sql"));
            assert!(!allowlist.iter().any(|t| t == "semantic_query"));
        }
    }

    // ── Smoke-test step in SemanticView prompt ──────────────────────────────

    #[test]
    fn semantic_view_prompt_includes_smoke_test_step() {
        // The smoke-test step is the safety net for date type-mismatch
        // and similar runtime errors that pass structural validation
        // but fail at first analytics query.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains("Smoke-test"),
            "SemanticView prompt must include a smoke-test step"
        );
        assert!(
            prompt.contains("semantic_query"),
            "SemanticView prompt must instruct calling semantic_query for the smoke test"
        );
        assert!(
            prompt.to_lowercase().contains("type_mismatch") || prompt.contains("TYPE_MISMATCH"),
            "SemanticView prompt must name the TYPE_MISMATCH failure mode"
        );
    }

    #[test]
    fn semantic_view_prompt_with_prefetched_schema_renumbers_smoke_test() {
        // The pre-fetched-schema path inlines the column list and skips the
        // DESCRIBE TABLE step, so downstream steps shift down by one:
        // Step 1 = create view, Step 2 = create topic, Step 3 = smoke test.
        // ctx_with()'s default `table_schema: None` exercises the
        // DESCRIBE-first path; this test pins the renumbered path so the
        // smoke-test step survives and no DESCRIBE instruction bleeds in.
        let mut ctx = ctx_with(OnboardingBuildStep::SemanticView);
        ctx.table_schema = Some(vec![
            TableColumnDef {
                name: "order_id".to_string(),
                column_type: "VARCHAR".to_string(),
            },
            TableColumnDef {
                name: "order_date".to_string(),
                column_type: "DATE".to_string(),
            },
            TableColumnDef {
                name: "amount".to_string(),
                column_type: "DECIMAL(10,2)".to_string(),
            },
        ]);
        let prompt = ctx.build_prompt();

        // Pre-fetched schema is inlined, DESCRIBE is skipped.
        assert!(
            prompt.contains("Table schema (pre-fetched)"),
            "pre-fetched schema section must be present"
        );
        assert!(
            !prompt.contains("DESCRIBE TABLE"),
            "DESCRIBE TABLE instruction must NOT appear when table_schema is supplied"
        );
        assert!(
            prompt.contains("`order_date` (DATE)"),
            "pre-fetched column list must be inlined verbatim"
        );

        // Step renumbering: view = 1, topic = 2, smoke test = 3.
        assert!(
            prompt.contains("## Step 1: Create the view file"),
            "view-creation should be Step 1 on the pre-fetched path"
        );
        assert!(
            prompt.contains("## Step 2: Create the topic file"),
            "topic-creation should be Step 2 on the pre-fetched path"
        );
        assert!(
            prompt.contains("## Step 3: Smoke-test the view"),
            "smoke-test must remain at the end (Step 3) on the pre-fetched path"
        );

        // Sanity: the smoke-test substance still references semantic_query
        // and the TYPE_MISMATCH failure mode (the safety net is intact).
        assert!(prompt.contains("semantic_query"));
        assert!(prompt.contains("TYPE_MISMATCH"));
    }

    #[test]
    fn semantic_view_prompt_warns_about_date_columns() {
        // The most common silent foot-gun: integer-encoded date columns.
        let prompt = ctx_with(OnboardingBuildStep::SemanticView).build_prompt();
        assert!(
            prompt.contains("date") && prompt.contains("type:"),
            "SemanticView prompt must warn about date-column type/expr handling"
        );
    }

    #[test]
    fn legacy_semantic_layer_prompt_includes_smoke_test_step() {
        // Same safety net for the legacy all-in-one phase.
        let prompt = ctx_with(OnboardingBuildStep::SemanticLayer).build_prompt();
        assert!(
            prompt.contains("Smoke-test") || prompt.contains("smoke-test"),
            "legacy SemanticLayer prompt must include smoke-test step"
        );
        assert!(prompt.contains("semantic_query"));
    }

    #[test]
    fn agent_prompt_does_not_require_pre_write_file_reads() {
        // Onboarding just created the view + topic files in the prior
        // phase; instructing the agent to "read at least one .topic.yml
        // and one .view.yml" before authoring the agentic file is
        // wasted tool calls.  The instruction has been replaced with a
        // direct "go straight to creating the agentic file".
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(
            !prompt.contains("Read at least one"),
            "Agent prompt should not require pre-write file reads"
        );
    }

    #[test]
    fn legacy_semantic_layer_prompt_points_at_reference_card() {
        // The legacy all-in-one phase is still the Default variant and can be
        // hit by older frontends or requests that omit the `step` field. It
        // must defer to the same cached reference card the per-phase path
        // does, so both branches produce schema-compliant views.
        let prompt = ctx_with(OnboardingBuildStep::SemanticLayer).build_prompt();
        assert!(
            prompt.contains("Semantic model reference"),
            "legacy SemanticLayer prompt must defer to the cached semantic-layer reference card"
        );
        assert!(
            prompt.contains(".topic.yml"),
            "legacy SemanticLayer prompt must instruct topic creation alongside views"
        );
    }

    // ── Data Apps are retired: onboarding never creates one ─────────────────

    const EVERY_STEP: [OnboardingBuildStep; 4] = [
        OnboardingBuildStep::SemanticLayer,
        OnboardingBuildStep::Config,
        OnboardingBuildStep::SemanticView,
        OnboardingBuildStep::Agent,
    ];

    #[test]
    fn no_onboarding_step_asks_for_a_data_app() {
        for step in EVERY_STEP {
            let ctx = ctx_with(step.clone());
            let prompt = ctx.build_prompt();
            assert!(
                !prompt.contains(".app.yml"),
                "phase {step:?} prompt mentions a Data App file"
            );
            assert!(
                !ctx.tool_allowlist().iter().any(|t| t == "run_app"),
                "phase {step:?} still exposes run_app"
            );
        }
    }

    #[test]
    fn the_agent_phase_ends_with_sample_questions_for_the_completion_screen() {
        // The completion screen's "Try these with your analytics agent" prompts come
        // from the last phase's answer. The starter-dashboard phase used to write
        // them; the agent phase does now.
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(prompt.contains("\"Sample Questions\" section with 5 numbered questions"));
        assert!(
            prompt.contains("- public.orders"),
            "the tables are named: {prompt}"
        );
    }

    #[test]
    fn the_generated_agent_does_not_read_apps() {
        let prompt = ctx_with(OnboardingBuildStep::Agent).build_prompt();
        assert!(!prompt.contains(".app.yml"));
    }

    #[test]
    fn the_app_steps_no_longer_deserialize() {
        for step in ["app", "app2"] {
            let body = format!(
                r#"{{"tables": ["public.orders"], "warehouse_type": "postgres", "step": "{step}"}}"#
            );
            assert!(
                serde_json::from_str::<OnboardingContext>(&body).is_err(),
                "step {step:?} was accepted: an old client would get a Data App prompt"
            );
        }
        let body =
            r#"{"tables": ["public.orders"], "warehouse_type": "postgres", "step": "agent"}"#;
        assert!(serde_json::from_str::<OnboardingContext>(body).is_ok());
    }
}
