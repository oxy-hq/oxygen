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
        metadata: None,
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
fn only_a_root_ask_run_waiting_on_input_warrants_a_notice() {
    assert!(warrants_notice(&ask_run("awaiting_input")));
    for status in ["running", "delegating", "done", "failed", "cancelled"] {
        assert!(!warrants_notice(&ask_run(status)), "{status}");
    }
    let mut r = ask_run("awaiting_input");
    r.cancel_requested_at = Some(Utc::now().fixed_offset());
    assert!(!warrants_notice(&r), "a cancel");
    let mut r = ask_run("awaiting_input");
    r.parent_run_id = Some("r-0".into());
    assert!(!warrants_notice(&r), "a child run");
    let mut r = ask_run("awaiting_input");
    r.thread_id = None;
    assert!(!warrants_notice(&r), "no thread");
    let mut r = ask_run("awaiting_input");
    r.source_type = Some("builder".into());
    assert!(!warrants_notice(&r), "another domain");
}

fn waiting(prompt: Option<&str>) -> AskWaitingRecord {
    AskWaitingRecord {
        workspace_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
        thread_id: Some(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()),
        prompt: prompt.map(str::to_string),
        question: "What was revenue last quarter?".into(),
    }
}

#[test]
fn the_body_is_the_first_prompt_and_the_link_opens_the_thread() {
    let r = words(
        &waiting(Some("Which region do you mean?\nAnd which quarter?")),
        Some("acme"),
    );
    assert_eq!(r.title, "Oxygen has a question for you");
    assert_eq!(r.body.as_deref(), Some("Which region do you mean?"));
    assert_eq!(
        r.href.as_deref(),
        Some(
            "/acme/workspaces/11111111-1111-1111-1111-111111111111/threads/\
             22222222-2222-2222-2222-222222222222"
        )
    );
}

#[test]
fn without_its_prompt_the_body_is_the_question_and_long_prompts_are_cut() {
    assert_eq!(
        words(&waiting(None), None).body.as_deref(),
        Some("What was revenue last quarter?")
    );
    assert_eq!(
        words(&waiting(Some("  \n ")), None).body.as_deref(),
        Some("What was revenue last quarter?")
    );
    let long = "x".repeat(300);
    let body = words(&waiting(Some(&long)), None).body.unwrap();
    assert_eq!(body.chars().count(), 120);
}
