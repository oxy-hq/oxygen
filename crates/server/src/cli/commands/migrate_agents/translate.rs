//! Classic `.agent.yml` fields → agentic `.agentic.yml` fields.
//!
//! The table this implements is § 4.2 of
//! `internal-docs/2026-11-09-classic-agent-retirement.md`. Every field lands in
//! exactly one bucket: **mapped** onto an agentic field, **dropped** because the
//! analytics pipeline owns that behaviour now (reported, not silent), or
//! **refused** because nothing in the agentic runtime does what it did. A file
//! with a refused field is not translated unless the caller forces the drop.
//! Unknown fields are refused too: a field this table has never seen is not
//! known to be safe to lose.

use std::collections::HashMap;

use serde_yaml::{Mapping, Value};

/// Context globs the agentic pipeline reads (memo § 4.2, `retrieval` row).
const CONTEXT_SUFFIXES: &[&str] = &[
    ".sql",
    ".view.yml",
    ".topic.yml",
    ".automation.yml",
    ".procedure.yml",
    ".md",
];

/// One translated (or refused) classic file.
#[derive(Debug, Default)]
pub(super) struct Translation {
    /// The `.agentic.yml` text. Empty when the YAML did not parse.
    pub yaml: String,
    /// Fields that have no agentic equivalent but whose behaviour the pipeline
    /// already owns, so losing them loses nothing a user configured on purpose.
    pub dropped: Vec<String>,
    /// Fields nothing in the agentic runtime does. Non-empty → not translated.
    pub refused: Vec<String>,
    /// What a reviewer should look at in the output.
    pub notes: Vec<String>,
    /// The classic `name:`, which was the agent's id. Refs by it are rewritten.
    pub name: Option<String>,
}

/// Topic files by stem, so `semantic_query.topic: x` can name `**/x.topic.yml`.
pub(super) type TopicIndex = HashMap<String, Vec<String>>;

#[derive(Default)]
struct Builder {
    llm_ref: Option<Value>,
    thinking: Option<Value>,
    databases: Vec<String>,
    context: Vec<String>,
    instructions: Option<String>,
    description: Option<String>,
    out: Translation,
}

/// Translate one classic agent. `rel_path` is only used in the header comment.
pub(super) fn translate(rel_path: &str, content: &str, topics: &TopicIndex) -> Translation {
    let map = match serde_yaml::from_str::<Value>(content) {
        Ok(Value::Mapping(map)) => map,
        Ok(_) => return unparseable("the file is not a YAML mapping"),
        Err(e) => return unparseable(&format!("the file is not valid YAML: {e}")),
    };
    let mut b = Builder::default();
    for (key, value) in &map {
        match key.as_str() {
            Some(key) => apply_field(&mut b, key, value, topics),
            None => push_unique(&mut b.out.refused, "<non-string key>".into()),
        }
    }
    b.finish(rel_path)
}

fn unparseable(why: &str) -> Translation {
    Translation {
        refused: vec!["<yaml>".into()],
        notes: vec![why.to_string()],
        ..Translation::default()
    }
}

fn apply_field(b: &mut Builder, key: &str, value: &Value, topics: &TopicIndex) {
    match key {
        "model" => b.llm_ref = Some(value.clone()),
        "system_instructions" => b.instructions = value.as_str().map(str::to_owned),
        "description" => b.description = value.as_str().map(str::to_owned),
        "name" => {
            b.out.name = scalar(value);
            b.drop("name");
        }
        "reasoning" => apply_reasoning(b, value),
        "tools" => seq(value).for_each(|t| apply_tool(b, t, topics)),
        "context" => seq(value).for_each(|c| apply_context(b, c)),
        "output_format" | "max_tool_calls" | "max_tool_concurrency" => b.drop(key),
        "tests" => {
            b.drop("tests");
            b.note("inline `tests:` were dropped; write them as a `.test.yml` file");
        }
        "type" if value.as_str() == Some("default") => b.drop("type"),
        "type" => b.refuse(&format!("type: {}", scalar(value).unwrap_or_default())),
        // anonymize, routes, route_fallback and everything unknown.
        other => b.refuse(other),
    }
}

/// `reasoning: { effort }` → `thinking: { effort }`.
fn apply_reasoning(b: &mut Builder, value: &Value) {
    match value.get("effort") {
        Some(effort) => {
            let mut m = Mapping::new();
            m.insert("effort".into(), effort.clone());
            b.thinking = Some(Value::Mapping(m));
        }
        None => b.drop("reasoning"),
    }
}

/// Sub-keys every classic tool carries that change nothing at runtime.
const TOOL_COMMON_KEYS: &[&str] = &["type", "name", "description"];

/// The sub-keys this table understands per tool type. Any other sub-key is
/// refused, as an unknown top-level field is: a setting the translation has
/// not reviewed (a limit, a filter, a credential) is never silently lost.
fn tool_keys(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "execute_sql" => &["database"],
        "visualize" | "create_data_app" => &[],
        "retrieval" => &["src"],
        "workflow" => &["workflow_ref", "output_task_ref"],
        "semantic_query" => &["topic", "variables"],
        _ => return None,
    })
}

fn apply_tool(b: &mut Builder, tool: &Value, topics: &TopicIndex) {
    let kind = tool
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<untyped>");
    let field = format!("tools[{kind}]");
    // omni_query, validate_sql and every tool the table does not know.
    // validate_sql in particular: `databases:` grants execution, and the
    // agentic runtime has no validate-only mode, so mapping it would widen
    // what the agent may do.
    let Some(known) = tool_keys(kind) else {
        return b.refuse(&field);
    };
    // Refuse the unknown sub-keys, then map the known ones regardless: under
    // `--force` exactly the refused sub-keys are dropped, not the whole tool.
    refuse_unknown_keys(b, tool, &field, known);
    match kind {
        "execute_sql" => match tool.get("database").and_then(scalar_ref) {
            Some(db) => push_unique(&mut b.databases, db),
            None => b.refuse(&field),
        },
        "visualize" | "create_data_app" => b.drop(&field),
        "retrieval" => {
            b.drop(&field);
            let src = tool.get("src").map(strings).unwrap_or_default();
            src.into_iter().for_each(|s| b.add_context(s));
        }
        "workflow" => apply_workflow(b, tool, &field),
        _ => apply_semantic_query(b, tool, topics, &field),
    }
}

/// Refuse every sub-key of `item` outside `known` and the common keys. The
/// caller still maps the known keys, so `--force` drops only what is named.
fn refuse_unknown_keys(b: &mut Builder, item: &Value, field: &str, known: &[&str]) {
    let Some(map) = item.as_mapping() else {
        return b.refuse(field);
    };
    for key in map.keys() {
        let key = key.as_str().unwrap_or("<non-string key>");
        if !TOOL_COMMON_KEYS.contains(&key) && !known.contains(&key) {
            b.refuse(&format!("{field}.{key}"));
        }
    }
}

/// `workflow_ref` → `context:`. The agent runs the whole automation it finds
/// in context, so `output_task_ref` (which task's output the tool returned)
/// is dropped and reported.
fn apply_workflow(b: &mut Builder, tool: &Value, field: &str) {
    match tool.get("workflow_ref").and_then(scalar_ref) {
        Some(path) => b.add_context(path),
        None => return b.refuse(field),
    }
    if tool.get("output_task_ref").is_some() {
        b.drop(&format!("{field}.output_task_ref"));
    }
}

/// `semantic_query.topic: x` → `context: [<the one x.topic.yml>]`.
fn apply_semantic_query(b: &mut Builder, tool: &Value, topics: &TopicIndex, field: &str) {
    let topic = tool.get("topic").and_then(scalar_ref).unwrap_or_default();
    match topics.get(&topic).map(Vec::as_slice) {
        Some([path]) => {
            b.add_context(path.clone());
            if tool.get("variables").is_some() {
                b.note("`semantic_query.variables` were dropped; the pipeline has none");
            }
        }
        found => {
            let why = if found.is_some() {
                "more than one"
            } else {
                "no"
            };
            b.refuse(field);
            b.note(&format!("{why} `{topic}.topic.yml` in the workspace"));
        }
    }
}

/// `context: [{type: file|semantic_model, src}]` → root-relative globs. The
/// classic `src:` was already workspace-root-relative, as agentic `context:` is.
fn apply_context(b: &mut Builder, item: &Value) {
    if !item.is_string() {
        let kind = item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("<untyped>");
        let field = format!("context[{kind}]");
        if !matches!(kind, "file" | "semantic_model") {
            return b.refuse(&field);
        }
        refuse_unknown_keys(b, item, &field, &["src"]);
    }
    match item.get("src").or(Some(item).filter(|i| i.is_string())) {
        Some(src) => strings(src).into_iter().for_each(|s| b.add_context(s)),
        None => b.refuse("context"),
    }
}

impl Builder {
    fn drop(&mut self, field: &str) {
        push_unique(&mut self.out.dropped, field.to_string());
    }
    fn refuse(&mut self, field: &str) {
        push_unique(&mut self.out.refused, field.to_string());
    }
    fn note(&mut self, note: &str) {
        push_unique(&mut self.out.notes, note.to_string());
    }
    /// Add a `context:` glob, unless it names a file type the agentic
    /// pipeline does not read (a classic `semantic_model` `.schema.yml`):
    /// `resolve_context` ignores those silently, so writing one would read as
    /// grounding the agent while giving it nothing. Reported instead.
    fn add_context(&mut self, glob: String) {
        if is_context_glob(&glob) {
            push_unique(&mut self.context, glob);
        } else {
            self.note(&format!(
                "`context` entry `{glob}` is not a file type the agentic pipeline reads \
                 (.sql/.view.yml/.topic.yml/.automation.yml/.md); it was dropped"
            ));
        }
    }

    fn finish(mut self, rel_path: &str) -> Translation {
        let instructions = self.instructions.as_deref().unwrap_or_default();
        if instructions.contains("{{") || instructions.contains("{%") {
            self.note(
                "instructions use Jinja (`{{ }}` / `{% %}`), which the agentic pipeline \
                 passes to the model literally; rewrite them",
            );
        }
        if self.llm_ref.is_none() {
            self.note("no `model:`; the agent uses the default model");
        }
        let body = serde_yaml::to_string(&Value::Mapping(self.body())).unwrap_or_default();
        self.out.yaml = format!("{}{body}", self.header(rel_path));
        self.out
    }

    fn header(&self, rel_path: &str) -> String {
        let mut h = format!("# Translated from {rel_path} by `oxy migrate-agents`.\n");
        for line in self.description.as_deref().unwrap_or_default().lines() {
            h.push_str(&format!("# {line}\n").replace("# \n", "#\n"));
        }
        h
    }

    fn body(&self) -> Mapping {
        let mut m = Mapping::new();
        if let Some(model) = &self.llm_ref {
            let mut llm = Mapping::new();
            llm.insert("ref".into(), model.clone());
            m.insert("llm".into(), Value::Mapping(llm));
        }
        if let Some(thinking) = &self.thinking {
            m.insert("thinking".into(), thinking.clone());
        }
        for (key, list) in [("databases", &self.databases), ("context", &self.context)] {
            if !list.is_empty() {
                let items = list.iter().cloned().map(Value::String).collect();
                m.insert(key.into(), Value::Sequence(items));
            }
        }
        if let Some(text) = &self.instructions {
            m.insert("instructions".into(), Value::String(text.clone()));
        }
        m
    }
}

/// A glob the pipeline can read from: it names a context suffix, or ends in a
/// wildcard (`semantics/**/*`) that may match one.
fn is_context_glob(src: &str) -> bool {
    CONTEXT_SUFFIXES.iter().any(|s| src.ends_with(s)) || src.ends_with('*')
}

fn seq(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_sequence().into_iter().flatten()
}

/// A string, or each string of a list.
fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::Sequence(items) => items.iter().filter_map(scalar).collect(),
        other => scalar(other).into_iter().collect(),
    }
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn scalar_ref(value: &Value) -> Option<String> {
    scalar(value).filter(|s| !s.trim().is_empty())
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}
