use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use agentic_analytics::config::yaml::AgentConfig;

use super::*;

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap()
}

fn migrate(root: &Path, dry_run: bool, force: bool) -> Report {
    run(&MigrateAgentsArgs {
        path: root.to_string_lossy().into_owned(),
        dry_run,
        json: false,
        force,
    })
    .unwrap()
}

/// Every file under `root`, with its content.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for entry in walkdir(root) {
        let rel = entry
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        out.insert(rel, fs::read_to_string(&entry).unwrap_or_default());
    }
    out
}

fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walkdir(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

const SQL_AGENT: &str = r#"# yaml-language-server: $schema=x
name: sales_bot
description: "Answers sales questions."
model: "openai-4o-mini"
reasoning:
  effort: low
context:
  - name: queries
    type: file
    src:
      - 'data/*.sql'
system_instructions: |
  You are an analyst.
  Dialect: {{ databases.local.dialect }}
tools:
  - name: execute_sql
    type: execute_sql
    database: local
  - name: report
    type: workflow
    workflow_ref: procedures/report.automation.yml
  - name: retrieval
    type: retrieval
    src:
      - 'data/*.sql'
      - 'docs/*.txt'
  - name: chart
    type: visualize
  - name: sq
    type: semantic_query
    topic: fruit
output_format: file
tests:
  - type: consistency
    task_description: "how many?"
"#;

#[test]
fn translates_mapped_fields_into_a_config_the_agentic_runtime_parses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "agents/sales.agent.yml", SQL_AGENT);
    write(
        root,
        "semantics/topics/fruit.topic.yml",
        "name: fruit\nviews: []\n",
    );

    let report = migrate(root, false, false);

    assert!(!root.join("agents/sales.agent.yml").exists());
    let yaml = read(root, "agents/sales.agentic.yml");
    assert!(
        yaml.starts_with("# Translated from agents/sales.agent.yml"),
        "{yaml}"
    );
    assert!(yaml.contains("# Answers sales questions."), "{yaml}");
    let config: AgentConfig = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(config.llm.model_ref.as_deref(), Some("openai-4o-mini"));
    assert_eq!(config.databases, vec!["local"]);
    assert_eq!(
        config.context,
        vec![
            "data/*.sql",
            "procedures/report.automation.yml",
            "semantics/topics/fruit.topic.yml",
        ]
    );
    assert!(config.thinking.is_some());
    assert!(config.instructions.unwrap().contains("You are an analyst."));

    let t = &report.translated[0];
    assert_eq!(t.to, "agents/sales.agentic.yml");
    for field in [
        "name",
        "tools[retrieval]",
        "tools[visualize]",
        "output_format",
        "tests",
    ] {
        assert!(
            t.dropped.contains(&field.to_string()),
            "{field}: {:?}",
            t.dropped
        );
    }
    assert!(t.notes.iter().any(|n| n.contains("Jinja")), "{:?}", t.notes);
    assert!(
        t.notes.iter().any(|n| n.contains("sales_bot")),
        "{:?}",
        t.notes
    );
    assert!(report.unmappable.is_empty());
}

#[test]
fn an_unmappable_field_is_refused_and_the_file_left_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let anon = "model: m\nanonymize:\n  type: flash_text\nsystem_instructions: hi\n";
    let omni = "model: m\ntools:\n  - type: omni_query\n    topic: Hire\n    integration: o\n";
    write(root, "agents/anon.agent.yml", anon);
    write(root, "agents/omni.agent.yml", omni);
    write(
        root,
        "procedures/a.automation.yml",
        "tasks:\n  - agent_ref: agents/anon.agent.yml\n",
    );

    let report = migrate(root, false, false);

    assert_eq!(read(root, "agents/anon.agent.yml"), anon);
    assert!(!root.join("agents/anon.agentic.yml").exists());
    assert!(!root.join("agents/omni.agentic.yml").exists());
    let fields: Vec<_> = report
        .unmappable
        .iter()
        .map(|u| (&u.path[..], &u.fields[..]))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("agents/anon.agent.yml", &["anonymize".to_string()][..]),
            (
                "agents/omni.agent.yml",
                &["tools[omni_query]".to_string()][..]
            ),
        ]
    );
    assert!(report.translated.is_empty());
    assert_eq!(report.refs_blocked.len(), 1, "{:?}", report.refs_blocked);
    assert!(read(root, "procedures/a.automation.yml").contains("anon.agent.yml"));
}

#[test]
fn force_drops_the_refused_field_and_translates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "agents/anon.agent.yml",
        "model: m\nanonymize:\n  type: flash_text\n",
    );

    let report = migrate(root, false, true);

    assert!(root.join("agents/anon.agentic.yml").exists());
    assert_eq!(report.translated[0].forced, vec!["anonymize"]);
    assert!(!read(root, "agents/anon.agentic.yml").contains("anonymize"));
}

#[test]
fn rewrites_refs_by_path_by_stem_and_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "agents/local.agent.yml", "name: helper\nmodel: m\n");
    let automation = "\
tasks:
  - name: by_path
    agent_ref: agents/local.agent.yml  # comment kept
  - name: by_dot_path
    agent_ref: \"./agents/local.agent.yml\"
  - name: by_stem_file
    agent_ref: local.agent.yml
  - name: by_name
    agent_ref: helper
  - name: by_stem
    agent_ref: local
";
    write(root, "procedures/p.automation.yml", automation);
    write(
        root,
        "config.yml",
        "mcp:\n  tools:\n    - agents/*.agent.yml\n",
    );

    let report = migrate(root, false, false);

    let out = read(root, "procedures/p.automation.yml");
    assert!(
        out.contains("agent_ref: agents/local.agentic.yml  # comment kept"),
        "{out}"
    );
    assert!(
        out.contains("agent_ref: \"./agents/local.agentic.yml\""),
        "{out}"
    );
    assert!(out.contains("agent_ref: local.agentic.yml\n"), "{out}");
    assert!(
        out.contains("agent_ref: agents/local.agentic.yml\n  - name: by_stem"),
        "{out}"
    );
    // A ref by stem already resolves to the agentic file: left as written.
    assert!(out.ends_with("agent_ref: local\n"), "{out}");
    assert!(read(root, "config.yml").contains("- agents/*.agentic.yml"));
    assert_eq!(
        report.refs_rewritten.len(),
        5,
        "{:?}",
        report.refs_rewritten
    );
    for r in report.refs_rewritten.iter().filter(|r| !r.to.contains('*')) {
        let candidates = oxy_compile::agent_ref_candidates(r.to.trim_start_matches("./"));
        assert!(
            candidates
                .iter()
                .any(|c| c == "local" || c == "agents/local.agentic.yml"),
            "{r:?} does not resolve"
        );
    }
}

#[test]
fn a_collision_is_skipped_and_nothing_is_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "agents/a.agent.yml", "model: m\n");
    write(root, "agents/a.agentic.yml", "llm:\n  ref: keep\n");
    // Same id `b` from two directories, and `c` taken by an agentic `name:`.
    write(root, "x/b.agent.yml", "model: m\n");
    write(root, "y/b.agent.yml", "model: m\n");
    write(root, "agents/c.agent.yml", "model: m\n");
    write(root, "other.agentic.yml", "name: c\n");
    write(root, "agents/ok.agent.yml", "model: m\n");

    let report = migrate(root, false, false);

    assert_eq!(read(root, "agents/a.agentic.yml"), "llm:\n  ref: keep\n");
    assert!(root.join("agents/a.agent.yml").exists());
    assert!(root.join("x/b.agent.yml").exists() && root.join("y/b.agent.yml").exists());
    assert!(root.join("agents/c.agent.yml").exists());
    let mut collided: Vec<_> = report.collisions.iter().map(|c| c.path.as_str()).collect();
    collided.sort();
    assert_eq!(
        collided,
        vec![
            "agents/a.agent.yml",
            "agents/c.agent.yml",
            "x/b.agent.yml",
            "y/b.agent.yml"
        ]
    );
    assert!(root.join("agents/ok.agentic.yml").exists());
}

#[test]
fn dry_run_writes_nothing_and_reports_the_same_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "agents/sales.agent.yml", SQL_AGENT);
    write(root, "semantics/topics/fruit.topic.yml", "name: fruit\n");
    write(root, "agents/anon.agent.yml", "model: m\nanonymize: {}\n");
    write(
        root,
        "procedures/p.automation.yml",
        "tasks:\n  - agent_ref: agents/sales.agent.yml\n",
    );
    let before = snapshot(root);

    let report = migrate(root, true, false);

    assert_eq!(snapshot(root), before);
    assert!(report.dry_run);
    assert_eq!(report.classic_files.len(), 2);
    assert_eq!(report.translated.len(), 1);
    assert_eq!(report.unmappable.len(), 1);
    assert_eq!(report.refs_rewritten.len(), 1);
    let json = serde_json::to_value(&report).unwrap();
    for key in [
        "classic_files",
        "translated",
        "unmappable",
        "refs_rewritten",
        "collisions",
    ] {
        assert!(json.get(key).is_some(), "{key} missing from {json}");
    }
}

#[test]
fn skip_dirs_and_test_files_are_not_classic_agents() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "node_modules/x/a.agent.yml", "model: m\n");
    write(root, ".oxy_state/a.agent.yml", "model: m\n");
    write(
        root,
        "tests/a.agent.test.yml",
        "target: agents/a.agent.yml\n",
    );

    let report = migrate(root, false, false);

    assert!(
        report.classic_files.is_empty(),
        "{:?}",
        report.classic_files
    );
}

#[test]
fn validate_sql_is_refused_because_databases_would_grant_execution() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let sql = "model: m\ntools:\n  - name: v\n    type: validate_sql\n    database: local\n";
    write(root, "agents/sql.agent.yml", sql);

    let report = migrate(root, false, false);

    assert_eq!(read(root, "agents/sql.agent.yml"), sql);
    assert!(!root.join("agents/sql.agentic.yml").exists());
    assert_eq!(report.unmappable[0].fields, vec!["tools[validate_sql]"]);

    // Forced, the tool is dropped: it does not become a `databases:` entry.
    let report = migrate(root, false, true);
    assert_eq!(report.translated[0].forced, vec!["tools[validate_sql]"]);
    let config: AgentConfig = serde_yaml::from_str(&read(root, "agents/sql.agentic.yml")).unwrap();
    assert!(config.databases.is_empty(), "{:?}", config.databases);
}

#[test]
fn an_unknown_sub_key_on_a_tool_or_context_item_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let agent = "\
model: m
tools:
  - name: q
    type: execute_sql
    database: local
    dry_run_limit: 100
context:
  - name: c
    type: file
    src: data/*.sql
    filter: secret
";
    write(root, "agents/a.agent.yml", agent);

    let report = migrate(root, true, false);

    assert_eq!(
        report.unmappable[0].fields,
        vec!["tools[execute_sql].dry_run_limit", "context[file].filter"]
    );
    assert!(report.translated.is_empty());

    // `--force` drops exactly the named sub-keys; the known keys still map.
    let report = migrate(root, false, true);

    assert_eq!(
        report.translated[0].forced,
        vec!["tools[execute_sql].dry_run_limit", "context[file].filter"]
    );
    let config: AgentConfig = serde_yaml::from_str(&read(root, "agents/a.agentic.yml")).unwrap();
    assert_eq!(config.databases, vec!["local"]);
    assert_eq!(config.context, vec!["data/*.sql"]);
}

#[test]
fn a_context_entry_the_pipeline_cannot_read_is_dropped_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let agent = "\
model: m
context:
  - name: model
    type: semantic_model
    src: data/x.schema.yml
  - name: q
    type: file
    src: data/*.sql
";
    write(root, "agents/a.agent.yml", agent);

    let report = migrate(root, false, false);

    let config: AgentConfig = serde_yaml::from_str(&read(root, "agents/a.agentic.yml")).unwrap();
    assert_eq!(config.context, vec!["data/*.sql"]);
    let notes = &report.translated[0].notes;
    assert!(
        notes
            .iter()
            .any(|n| n.contains("`data/x.schema.yml`") && n.contains("dropped")),
        "{notes:?}"
    );
}
