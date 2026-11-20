use agentic_core::react::{Effect, Tool, ToolCx};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::super::AskUser;
use super::super::testkit::NoResults;

fn cx() -> ToolCx {
    ToolCx {
        run_id: "run1".into(),
        call_id: "c1".into(),
        cancel: CancellationToken::new(),
        results: std::sync::Arc::new(NoResults),
    }
}

#[tokio::test]
async fn ask_user_returns_the_suspend_value() {
    let args = json!({"question": "Which year?", "suggestions": ["2024"]});
    let reply = AskUser.call(args, &cx()).await.unwrap();
    assert_eq!(
        reply.value,
        json!({"question": "Which year?", "suggestions": ["2024"]})
    );
    assert_eq!(AskUser.effect(), Effect::Suspend);
}

#[tokio::test]
async fn ask_user_without_suggestions_gives_an_empty_list() {
    let reply = AskUser
        .call(json!({"question": "Why?"}), &cx())
        .await
        .unwrap();
    assert_eq!(reply.value, json!({"question": "Why?", "suggestions": []}));
}

#[tokio::test]
async fn ask_user_without_a_question_fails_with_a_hint() {
    for args in [json!({}), json!({"question": "  "})] {
        let err = AskUser.call(args, &cx()).await.unwrap_err();
        assert_eq!(err.hint.as_deref(), Some("pass a question string"));
    }
}
