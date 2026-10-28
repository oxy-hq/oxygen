//! End-to-end test: "resume only unchanged steps."
//!
//! Drives the full DB-backed decision loop (load state, decide, commit) to
//! verify that:
//!
//! 1. A successful run populates `step_hashes` for every step.
//! 2. A retry that points at the prior run with `cache_enabled = true`
//!    reuses every step's prior result and emits `subrun_step_cache_hit`
//!    for each.
//! 3. When a single step's config changes, that step (and every downstream
//!    step) re-executes — cascade is implicit because `render_context`
//!    feeds the hash.
//!
//! Run:
//!   cargo nextest run -p agentic-pipeline --test integration -E 'test(automation_cache_resume_test)'

use std::collections::HashMap;

use agentic_automation::AutomationDecider;
use agentic_automation::config::{
    AutomationConfig as WfConfig, FormatterConfig, TaskConfig, TaskType,
};
use agentic_automation::extension::{
    AutomationMigrator, DecisionClaim, DecisionCommit, DecisionTerminal, commit_decision,
    insert_automation_state, load_automation_state,
};
use agentic_runtime::crud;
use agentic_runtime::migration::RuntimeMigrator;
use sea_orm::{Database, DatabaseConnection};
use serde_json::{Value, json};

static TEST_DB_URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
static TEST_CONTAINER: tokio::sync::OnceCell<
    std::sync::Arc<testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>>,
> = tokio::sync::OnceCell::const_new();

async fn test_db() -> Option<DatabaseConnection> {
    let url = TEST_DB_URL
        .get_or_init(|| async {
            if let Ok(url) = std::env::var("OXY_DATABASE_URL") {
                return url;
            }
            use testcontainers::runners::AsyncRunner;
            use testcontainers::{ImageExt, ReuseDirective};
            use testcontainers_modules::postgres::Postgres;
            let container = TEST_CONTAINER
                .get_or_init(|| async {
                    std::sync::Arc::new(
                        Postgres::default()
                            // 64 MB (Docker default) is too small: a parallel plan wants a 32 MB
                            // DSM segment and a REUSED container accumulates them.
                            // Must match at every setup site — reuse hashes the config.
                            // See internal-docs/workspace-source.md.
                            .with_shm_size(1024 * 1024 * 1024)
                            .with_reuse(ReuseDirective::Always)
                            .start()
                            .await
                            .expect("start postgres"),
                    )
                })
                .await;
            let host = container.get_host().await.expect("postgres host");
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("postgres port");
            format!("postgres://postgres:postgres@{host}:{port}/postgres")
        })
        .await;
    let db = Database::connect(url).await.ok()?;
    // Central then runtime (production order — see oxy_test_utils::migration).
    oxy_test_utils::migration::migrate_shared_test_db::<RuntimeMigrator>(url, &db)
        .await
        .expect("shared migrate")
        .then::<AutomationMigrator>()
        .await
        .expect("automation migrate")
        .finish()
        .await;
    Some(db)
}

/// Build a 3-step automation of dependent formatters.
///
/// step1: emits `"a"`.
/// step2: emits `"<step1_text>-<seed>"` (depends on step1's render context).
/// step3: emits `"final-<step2_text>"` (depends on step2's render context).
fn make_automation(step2_seed: &str) -> WfConfig {
    WfConfig {
        name: "cache-test".into(),
        description: String::new(),
        variables: None,
        consistency_prompt: None,
        consistency_model: None,
        tasks: vec![
            TaskConfig {
                name: "step1".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: "a".into(),
                }),
            },
            TaskConfig {
                name: "step2".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: format!("{{{{ step1.text }}}}-{step2_seed}"),
                }),
            },
            TaskConfig {
                name: "step3".into(),
                task_type: TaskType::Formatter(FormatterConfig {
                    template: "final-{{ step2.text }}".into(),
                }),
            },
        ],
    }
}

async fn seed_run(
    db: &DatabaseConnection,
    automation: WfConfig,
    retry_from: Option<&str>,
    cache_enabled: bool,
) -> String {
    let run_id = format!("cache-{}", uuid::Uuid::new_v4());
    crud::insert_run(
        db,
        &run_id,
        "cache test",
        None,
        "workflow",
        None,
        uuid::Uuid::nil(),
    )
    .await
    .expect("insert run");
    let yaml_hash = agentic_automation::hash::canonical_hash(&automation).expect("hash");
    let state = agentic_automation::extension::AutomationRunState {
        run_id: run_id.clone(),
        workflow: automation,
        workflow_yaml_hash: yaml_hash,
        workflow_context: json!({"workspace_path": "/tmp"}),
        variables: None,
        trace_id: format!("trace-{}", uuid::Uuid::new_v4()),
        current_step: 0,
        results: HashMap::new(),
        render_context: json!({}),
        pending_children: HashMap::new(),
        decision_version: 0,
        step_hashes: HashMap::new(),
        retry_from_run_id: retry_from.map(str::to_string),
        cache_enabled,
        prior_step_hashes: HashMap::new(),
        prior_results: HashMap::new(),
        initial_render_context: json!({}),
        invalidate_iterations: HashMap::new(),
    };
    insert_automation_state(db, &state)
        .await
        .expect("insert state");
    run_id
}

/// Drive the decider + commit loop until the automation terminates, returning
/// the list of `subrun_step_cache_hit` step names emitted along the way.
async fn drive_to_complete(db: &DatabaseConnection, run_id: &str) -> Vec<String> {
    let mut cache_hits: Vec<String> = Vec::new();
    let decider = AutomationDecider::new(None);

    for _ in 0..32 {
        let state = load_automation_state(db, run_id)
            .await
            .expect("load")
            .expect("state present");
        let expected_version = state.decision_version;

        let pre_keys: std::collections::HashSet<String> = state.results.keys().cloned().collect();
        let pre_hash_keys: std::collections::HashSet<String> =
            state.step_hashes.keys().cloned().collect();

        let prior_owned = if state.cache_enabled
            && let Some(ref prior_id) = state.retry_from_run_id
        {
            load_automation_state(db, prior_id)
                .await
                .expect("load prior")
        } else {
            None
        };

        let (new_state, decision) = decider.decide(state, None, prior_owned.as_ref()).await;

        // Capture cache_hit events from this decision before consuming.
        let events_iter: &[(String, Value)] = match &decision {
            agentic_automation::AutomationDecision::Complete { emitted_events, .. }
            | agentic_automation::AutomationDecision::StepExecutedInline {
                emitted_events, ..
            } => emitted_events.as_slice(),
            _ => &[],
        };
        for (t, p) in events_iter {
            if t == "subrun_step_cache_hit"
                && let Some(name) = p.get("step").and_then(|v| v.as_str())
            {
                cache_hits.push(name.to_string());
            }
        }

        let result_delta: Value = new_state
            .results
            .iter()
            .filter(|(k, _)| !pre_keys.contains(*k))
            .map(|(k, v)| json!({ k.clone(): v }))
            .next()
            .unwrap_or(json!({}));
        let step_hash_delta: Value = new_state
            .step_hashes
            .iter()
            .filter(|(k, _)| !pre_hash_keys.contains(*k))
            .map(|(k, v)| json!({ k.clone(): v }))
            .next()
            .unwrap_or(json!({}));
        let events_vec = events_iter.to_vec();

        let terminal = match &decision {
            agentic_automation::AutomationDecision::Complete { final_answer, .. } => {
                DecisionTerminal::CompleteAutomation {
                    final_answer: final_answer.clone(),
                }
            }
            agentic_automation::AutomationDecision::Fail { error, .. } => {
                DecisionTerminal::FailAutomation {
                    error: error.clone(),
                }
            }
            _ => DecisionTerminal::Continuing,
        };

        // Inline / Complete decisions persist via commit_decision; other
        // variants (Delegate*) require a child outcome we don't simulate here.
        // The 3-formatter automation only ever produces Inline/Complete, so a
        // panic on anything else is a real bug.
        match decision {
            agentic_automation::AutomationDecision::StepExecutedInline { .. }
            | agentic_automation::AutomationDecision::Complete { .. } => {}
            other => panic!("unexpected non-inline decision: {other:?}"),
        }

        commit_decision(
            db,
            DecisionCommit {
                run_id: run_id.to_string(),
                decision_task_id: run_id.to_string(),
                claim: DecisionClaim::Unclaimed,
                expected_version,
                new_state,
                result_delta,
                step_hash_delta,
                events: events_vec,
                attempt: 0,
                terminal: matches_terminal(&terminal),
            },
        )
        .await
        .expect("commit");

        if matches!(terminal, DecisionTerminal::CompleteAutomation { .. }) {
            return cache_hits;
        }
    }
    panic!("automation did not complete within 32 decisions");
}

fn matches_terminal(t: &DecisionTerminal) -> DecisionTerminal {
    match t {
        DecisionTerminal::Continuing => DecisionTerminal::Continuing,
        DecisionTerminal::CompleteAutomation { final_answer } => {
            DecisionTerminal::CompleteAutomation {
                final_answer: final_answer.clone(),
            }
        }
        DecisionTerminal::FailAutomation { error } => DecisionTerminal::FailAutomation {
            error: error.clone(),
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_run_populates_step_hashes() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let automation = make_automation("v1");
    let run_id = seed_run(&db, automation, None, false).await;
    let cache_hits = drive_to_complete(&db, &run_id).await;
    assert!(cache_hits.is_empty(), "fresh run must not cache_hit");

    let final_state = load_automation_state(&db, &run_id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(final_state.results.len(), 3, "all steps produced results");
    assert_eq!(
        final_state.step_hashes.len(),
        3,
        "all successful steps recorded their hash, got: {:?}",
        final_state.step_hashes,
    );
    for name in ["step1", "step2", "step3"] {
        assert!(
            final_state.step_hashes.contains_key(name),
            "missing hash for {name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unchanged_retry_reuses_every_step() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let automation = make_automation("v1");
    let prior_id = seed_run(&db, automation.clone(), None, false).await;
    drive_to_complete(&db, &prior_id).await;

    // Same automation, retry from prior, opt-in caching.
    let new_id = seed_run(&db, automation, Some(&prior_id), true).await;
    let cache_hits = drive_to_complete(&db, &new_id).await;

    assert_eq!(
        cache_hits,
        vec![
            "step1".to_string(),
            "step2".to_string(),
            "step3".to_string()
        ],
        "every step must cache_hit when nothing changed",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn step_change_invalidates_self_and_downstream() {
    let Some(db) = test_db().await else {
        eprintln!("skipping: no DB available");
        return;
    };

    let prior_id = seed_run(&db, make_automation("v1"), None, false).await;
    drive_to_complete(&db, &prior_id).await;

    // step2's seed changes → step2 hash differs → step2 re-executes →
    // step2's output differs → step3's render_context differs → step3 hash
    // differs → step3 re-executes. step1 unchanged → cache hit.
    let new_id = seed_run(&db, make_automation("v2"), Some(&prior_id), true).await;
    let cache_hits = drive_to_complete(&db, &new_id).await;

    assert_eq!(
        cache_hits,
        vec!["step1".to_string()],
        "only step1 should cache_hit; step2 and step3 must re-execute",
    );

    // The new run produced a different step3 result reflecting the new step2 value.
    let final_state = load_automation_state(&db, &new_id)
        .await
        .expect("load")
        .expect("present");
    let step3 = final_state
        .results
        .get("step3")
        .expect("step3 result")
        .clone();
    assert_eq!(
        step3,
        json!({"text": "final-a-v2"}),
        "step3 must reflect step2's new output through render_context",
    );
}
