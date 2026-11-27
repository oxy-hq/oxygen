//! `POST …/checks/changes/seen`: a person's look at what a build changes.

use oxy_auth::user::LOCAL_GUEST_EMAIL;

use super::*;
use crate::promote_rollback::audit_rows;

const SEEN: &str = "app.build_changes_seen";

async fn mark(agent: &Agent, build: Uuid, body: Value) -> (StatusCode, Value) {
    let uri = format!("{}/changes/seen", checks(agent, build));
    person(agent, "POST", &uri, Some(body)).await
}

#[tokio::test]
async fn a_person_marks_the_changed_screens_seen_once_and_it_is_audited() {
    let agent = gated().await;
    let built = draft(&agent, "ps-1", None).await;

    // The comparison has not finished: nothing to look at yet.
    let (status, early) = mark(&agent, built, json!({ "attempt": 1 })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{early}");
    assert_eq!(early["code"], "nothing_to_see");
    pass_staging(&agent, built, json!([])).await;

    // The checker never marks anything seen, by this route or by a report.
    let uri = format!("{}/changes/seen", checks(&agent, built));
    let (status, _) = checker("POST", &uri, Some(json!({ "attempt": 1 }))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let forged = json!({ "attempt": 1, "checks": [passed("changes",
        json!({ "compared": 2, "changed": 1, "seen": { "by": "runner", "at": "now" } }))] });
    let report = format!("{}/report", checks(&agent, built));
    checker("POST", &report, Some(forged)).await;
    let changes = |rows: Vec<app_build_checks::Model>| {
        rows.into_iter()
            .find(|row| row.kind == "changes")
            .expect("the changes row")
    };
    let staging = key(built, "staging", 1);
    assert!(changes(rows(&agent, &staging).await).detail.unwrap()["seen"].is_null());
    assert!(audit_rows(&agent, SEEN).await.is_empty());

    let (status, marked) = mark(&agent, built, json!({ "attempt": 1 })).await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    let at = marked["seen"]["at"].as_str().expect("when").to_string();
    // A UTC instant in the form a check's other times are answered in.
    assert!(
        at.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(&at).is_ok(),
        "{at}"
    );
    assert_eq!(
        marked,
        json!({ "attempt": 1, "changed": 1, "compared": 2,
            "seen": { "by": LOCAL_GUEST_EMAIL, "at": at } })
    );
    let stored = changes(rows(&agent, &staging).await).detail.unwrap();
    assert_eq!(stored["seen"], marked["seen"], "on the changes row");
    assert_eq!(
        stored["changed"], 1,
        "the rest of the detail is as the runner sent it"
    );
    assert_eq!(stored["screens"].as_array().unwrap().len(), 2);
    let audited = audit_rows(&agent, SEEN).await;
    assert_eq!(audited.len(), 1);
    let said = audited[0].metadata.clone();
    assert_eq!(
        (
            &said["build"],
            &said["attempt"],
            &said["changed"],
            &said["environment"]
        ),
        (&json!("ps-1"), &json!(1), &json!(1), &json!("staging"))
    );

    // Marked again, with no attempt named: the first mark stands, unaudited.
    let (status, again) = mark(&agent, built, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again, marked);
    assert_eq!(audit_rows(&agent, SEEN).await.len(), 1);

    // The write itself keeps the first mark: a second writer that read the
    // row before the first wrote finds nothing to write.
    let row = changes(rows(&agent, &staging).await);
    let late = json!({ "by": "someone-else", "at": "later" });
    let wrote =
        oxy_api_custom_apps::custom_apps_build_checks::store::mark_seen(&agent.t.db, row.id, late)
            .await
            .expect("the write runs");
    assert!(!wrote, "a second mark replaced the first");
    let kept = changes(rows(&agent, &staging).await).detail.unwrap();
    assert_eq!(kept["seen"], marked["seen"]);

    // The promote plan reads the mark.
    let body = json!({ "from": "staging", "dry_run": true });
    let (_, plan) = crate::promote_rollback::promote(&agent, "production", body).await;
    assert_eq!(
        plan["checks"]["changes"],
        json!({ "changed": 1, "compared": 2, "seen": true, "seen_required": false })
    );
}

#[tokio::test]
async fn there_is_nothing_to_see_when_no_screen_changed_or_nothing_was_compared() {
    let agent = gated().await;
    let built = draft(&agent, "ps-2", None).await;
    let base = checks(&agent, built);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let same = json!({ "attempt": 1, "environment": "staging", "checks": [
        passed("changes", json!({ "compared": 2, "changed": 0, "screens": [], "seen": null })),
    ] });
    checker("POST", &format!("{base}/report"), Some(same)).await;
    let (status, refused) = mark(&agent, built, json!({ "attempt": 1 })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "nothing_to_see");
    assert_eq!(
        refused["message"],
        "nothing to mark as seen: no screen differs from production"
    );
    let (status, _) = mark(&agent, built, json!({ "attempt": 7 })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no such attempt");
    assert!(audit_rows(&agent, SEEN).await.is_empty());
}
