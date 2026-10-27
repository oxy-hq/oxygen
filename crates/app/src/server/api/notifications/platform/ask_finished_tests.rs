use chrono::Utc;

use super::*;

fn ask_run(status: &str) -> run::Model {
    let now = Utc::now().fixed_offset();
    run::Model {
        id: "r-1".into(),
        question: "What was revenue last quarter?".into(),
        answer: None,
        error_message: None,
        thread_id: Some(Uuid::new_v4()),
        source_type: Some("analytics".into()),
        metadata: Some(serde_json::json!({ "agent_id": "agents/a.agentic.yml" })),
        parent_run_id: None,
        schedule_id: None,
        task_status: Some(status.into()),
        task_metadata: None,
        attempt: 0,
        recovery_requested_at: None,
        driver_id: None,
        driver_heartbeat_at: None,
        cancel_requested_at: None,
        workspace_id: Uuid::nil(),
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn a_done_failed_or_timed_out_root_ask_run_on_a_thread_warrants_a_notice() {
    for status in ["done", "failed", "timed_out"] {
        assert!(warrants_notice(&ask_run(status)), "{status}");
    }
}

#[test]
fn a_run_still_going_or_waiting_for_an_answer_does_not() {
    for status in ["running", "awaiting_input", "delegating", "cancelled"] {
        assert!(!warrants_notice(&ask_run(status)), "{status}");
    }
}

#[test]
fn cancels_children_previews_threadless_and_other_domains_do_not() {
    let mut r = ask_run("failed");
    r.cancel_requested_at = Some(Utc::now().fixed_offset());
    assert!(!warrants_notice(&r), "a cancel recorded as failed");
    let mut r = ask_run("failed");
    r.error_message = Some("cancelled by user".into());
    assert!(!warrants_notice(&r), "a cancel without the flag");
    let mut r = ask_run("done");
    r.parent_run_id = Some("r-0".into());
    assert!(!warrants_notice(&r), "a child run");
    let mut r = ask_run("done");
    r.metadata = Some(serde_json::json!({
        agentic_runtime::crud::PREVIEW_STAMP_KEY: { "revision_id": "x" },
    }));
    assert!(!warrants_notice(&r), "a preview's run");
    let mut r = ask_run("done");
    r.thread_id = None;
    assert!(!warrants_notice(&r), "no thread, so no one asked");
    for source in ["builder", "workflow", "airway"] {
        let mut r = ask_run("done");
        r.source_type = Some(source.into());
        assert!(!warrants_notice(&r), "{source}");
    }
}

fn record(question: &str) -> AskRecord {
    AskRecord {
        workspace_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        thread_id: Some(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()),
        question: question.into(),
    }
}

const FINISHED: &str = super::super::kinds::ASK_FINISHED;
const FAILED: &str = super::super::kinds::ASK_FAILED;

#[test]
fn an_answer_reads_ready_a_failure_could_not_and_both_open_the_thread() {
    let ready = words(
        &record("What was revenue last quarter?"),
        FINISHED,
        Some("acme"),
    );
    assert_eq!(ready.title, "Your answer is ready");
    assert_eq!(
        ready.body.as_deref(),
        Some("What was revenue last quarter?")
    );
    assert_eq!(
        ready.href.as_deref(),
        Some(
            "/acme/workspaces/11111111-1111-1111-1111-111111111111/threads/\
             22222222-2222-2222-2222-222222222222"
        )
    );
    let failed = words(
        &record("What was revenue last quarter?"),
        FAILED,
        Some("acme"),
    );
    assert_eq!(failed.title, "Oxygen couldn't answer that");
    assert_eq!((failed.body, failed.href), (ready.body, ready.href));
    // A notice whose run is gone keeps its kind's title.
    let gone = super::super::render::fallback(FAILED);
    assert_eq!(
        (gone.title.as_str(), gone.href),
        ("Oxygen couldn't answer that", None)
    );
    assert_eq!(
        super::super::render::fallback(FINISHED).title,
        "Your answer is ready"
    );
}

#[test]
fn the_body_is_the_question_cut_to_120_characters() {
    let exact = "q".repeat(120);
    assert_eq!(
        words(&record(&exact), FINISHED, None).body,
        Some(exact.clone())
    );
    // Multi-byte characters count as one each.
    let long = format!("{} tail", "é".repeat(130));
    let body = words(&record(&long), FINISHED, None).body.unwrap();
    assert_eq!(body.chars().count(), 120);
    assert!(body.ends_with('…'));
    assert!(body.starts_with("éé"));
    assert_eq!(
        words(&record("  spaced  "), FINISHED, None).body.as_deref(),
        Some("spaced")
    );
    assert_eq!(words(&record("   "), FINISHED, None).body, None);
}

#[test]
fn without_the_org_or_the_thread_there_is_no_link() {
    assert_eq!(words(&record("q"), FINISHED, None).href, None);
    let mut no_thread = record("q");
    no_thread.thread_id = None;
    assert_eq!(words(&no_thread, FINISHED, Some("acme")).href, None);
}
