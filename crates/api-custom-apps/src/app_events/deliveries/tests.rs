use super::*;

#[test]
fn outcomes_are_the_columns_values() {
    let all = [
        AttemptOutcome::Ok,
        AttemptOutcome::Error,
        AttemptOutcome::Timeout,
        AttemptOutcome::Crash,
        AttemptOutcome::NonRetryable,
    ];
    let stored: Vec<&str> = all.iter().map(|o| o.as_str()).collect();
    assert_eq!(stored, ["ok", "error", "timeout", "crash", "non_retryable"]);
}

#[test]
fn delivery_info_is_ctx_delivery() {
    let info = DeliveryInfo {
        attempt: 5,
        max_attempts: 5,
        listener: "billing/on-upload".into(),
    };
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        serde_json::json!({ "attempt": 5, "maxAttempts": 5, "listener": "billing/on-upload" })
    );
    assert!(info.is_last());
    assert!(!DeliveryInfo { attempt: 4, ..info }.is_last());
}
