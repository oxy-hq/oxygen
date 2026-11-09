use super::*;
use crate::compile::{AutomationRefusals, compile_files};
use crate::walker::discover;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const AUTOMATION: &str = "name: nightly\ntasks:\n  - name: ask\n    type: agent\n    \
                          agent_ref: AGENT_REF\n    prompt: hi\n";

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn automation_with(agent_ref: &str) -> String {
    AUTOMATION.replace("AGENT_REF", agent_ref)
}

async fn warnings(root: &Path) -> (Vec<ClassicAgentWarning>, usize, u32) {
    let files = discover(root).unwrap();
    let batch = compile_files(
        &files,
        AutomationRefusals {
            retired_steps: false,
            workspace_writes: true,
        },
    )
    .await;
    (batch.warnings, batch.failures.len(), batch.compiled)
}

#[tokio::test]
async fn a_classic_file_warns_and_compiles_to_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(
        root,
        "agents/sales.agent.yml",
        "model: gpt\ntools:\n  - type: execute_sql\n  - type: retrieval\n",
    );
    // Test mirrors are not classic agents.
    write(root, "agents/sales.agent.test.yml", "name: t\n");
    write(root, "agents/sales.test.agent.yml", "name: t\n");
    write(root, "node_modules/x/old.agent.yml", "model: gpt\n");

    let (warned, failures, compiled) = warnings(root).await;
    assert_eq!(failures, 0, "a warning is not a refusal");
    assert_eq!(compiled, 0, "a classic file compiles to no row");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert_eq!(warned[0].path, "agents/sales.agent.yml");
    let m = &warned[0].message;
    assert!(m.contains("retired"), "{m}");
    assert!(m.contains("nothing runs"), "{m}");
    assert!(m.contains("oxy migrate-agents"), "{m}");
    assert!(m.contains("tools[execute_sql, retrieval]"), "{m}");
}

#[tokio::test]
async fn an_agent_ref_naming_a_classic_file_warns_even_when_the_file_exists() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "agents/sales.agent.yml", "model: gpt\n");
    write(
        root,
        "flows/nightly.automation.yml",
        &automation_with("agents/sales.agent.yml"),
    );

    let (warned, failures, compiled) = warnings(root).await;
    assert_eq!(failures, 0);
    assert_eq!(compiled, 1, "the automation still compiles");
    let step = warned
        .iter()
        .find(|w| w.path == "flows/nightly.automation.yml")
        .expect("the automation is warned about");
    assert!(step.message.contains("tasks[0]"), "{}", step.message);
    assert!(step.message.contains("classic"), "{}", step.message);
}

#[tokio::test]
async fn an_agent_ref_with_no_compiled_agent_behind_it_warns() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "agents/real.agentic.yml", "name: real\n");
    write(
        root,
        "flows/nightly.automation.yml",
        &automation_with("agents/missing.agentic.yml"),
    );

    let (warned, _, _) = warnings(root).await;
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(
        warned[0].message.contains("names no agent that compiled"),
        "{}",
        warned[0].message
    );
}

#[tokio::test]
async fn an_agentic_agent_referenced_correctly_produces_no_warning() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "agents/sales.agentic.yml", "llm:\n  ref: m\n");
    write(root, "agents/other.agentic.yml", "name: custom-key\n");
    // Path, path minus extension, and the `name:` key all resolve.
    for (i, r) in ["agents/sales.agentic.yml", "agents/sales", "custom-key"]
        .iter()
        .enumerate()
    {
        write(
            root,
            &format!("flows/f{i}.automation.yml"),
            &automation_with(r),
        );
    }
    write(
        root,
        "flows/builder.automation.yml",
        &automation_with("__builder__"),
    );

    let (warned, failures, compiled) = warnings(root).await;
    assert!(warned.is_empty(), "{warned:?}");
    assert_eq!(failures, 0);
    assert_eq!(compiled, 6);
}

#[test]
fn nested_steps_are_checked() {
    let nested = "name: n\ntasks:\n  - type: loop_sequential\n    tasks:\n      - type: agent\n        \
                  agent_ref: a.agent.yml\n  - type: conditional\n    else:\n      - type: agent\n        \
                  agent_ref: b.agent.yml\n";
    let row = CompiledRow::Automation(CompiledAutomation {
        file_path: "n.automation.yml".into(),
        name: "n".into(),
        extension: "automation".into(),
        definition: serde_yaml::from_str(nested).unwrap(),
    });
    let warned = agent_ref_warnings(&[row]);
    assert_eq!(warned.len(), 2, "{warned:?}");
    assert!(warned[0].message.contains("tasks[0].tasks[0]"));
    assert!(warned[1].message.contains("tasks[1].else[0]"));
}

#[tokio::test]
async fn refs_resolve_the_way_the_runtime_resolves_them() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "agents/sales.agentic.yml", "name: sales\n");
    for (i, r) in [
        "sales.agentic.yml",
        "x/sales.agentic.yml",
        "agents/sales.agentic.yaml",
        "sales",
    ]
    .iter()
    .enumerate()
    {
        write(
            root,
            &format!("flows/f{i}.automation.yml"),
            &automation_with(r),
        );
    }
    let (warned, failures, _) = warnings(root).await;
    assert!(warned.is_empty(), "{warned:?}");
    assert_eq!(failures, 0);
}

#[tokio::test]
async fn a_stem_that_matches_nothing_warns() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "agents/sales.agentic.yml", "name: sales\n");
    write(
        root,
        "flows/f.automation.yml",
        &automation_with("x/other.agentic.yml"),
    );
    let (warned, _, _) = warnings(root).await;
    assert_eq!(warned.len(), 1, "{warned:?}");
}

#[test]
fn candidates_are_full_path_bare_path_and_stem() {
    assert_eq!(
        agent_ref_candidates("./agents/a.agentic.yaml"),
        ["agents/a.agentic.yaml", "agents/a", "a"]
    );
    assert_eq!(agent_ref_candidates("a"), ["a"]);
}
