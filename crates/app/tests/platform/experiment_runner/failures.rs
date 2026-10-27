use chrono::Duration;

use super::fixture::*;

#[tokio::test]
async fn missing_lever_automation_fails_the_flip_loudly() {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    // `promo_on` is renamed after registration.
    fx.promote(
        utc("2026-02-20T00:00:00Z"),
        DESIGN,
        &["promo_start", "promo_off"],
    )
    .await;
    assert_eq!(
        fx.tick(switch_at("2026-03-02")).await,
        1,
        "the tick completes"
    );

    for unit in ["bondi", "newtown"] {
        let f = fx.flip(unit, "on").await;
        assert_eq!(f.status, "failed");
        assert!(f.task_id.is_none());
        assert!(
            f.lever_revision_id.is_none(),
            "nothing resolved, nothing recorded"
        );
        let reason = f.reason.expect("a reason");
        assert!(
            reason.contains("promo_on"),
            "PR 3's refusal names the lever: {reason}"
        );
        assert_eq!(fx.lever_runs(unit, "on").await, 0);
    }
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 2, "one per failed flip");
    assert!(notices.iter().all(|n| n.user_id == fx.owner));
    assert!(
        notices
            .iter()
            .all(|n| n.body.as_deref().unwrap().contains("due 2026-03-02")),
        "a notice names the flip's due date: {notices:?}"
    );
    assert_eq!(fx.run().await.state, "live", "the run continues");

    // The lever comes back; the next wave dispatches; nothing repeats.
    fx.promote(utc("2026-03-10T00:00:00Z"), DESIGN, LEVERS)
        .await;
    fx.tick(switch_at("2026-03-16")).await;
    assert_eq!(fx.lever_runs("manly", "on").await, 1);
    assert_eq!(
        fx.flip("bondi", "on").await.status,
        "failed",
        "a failed flip is not retried"
    );
    assert_eq!(fx.notices().await.len(), 2);
}

#[tokio::test]
async fn a_lever_run_that_fails_marks_its_flip_failed_and_tells_the_owner() {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    fx.tick(switch_at("2026-03-02")).await;
    fx.finish_levers("failed").await;
    fx.tick(switch_at("2026-03-02") + Duration::minutes(5))
        .await;
    let f = fx.flip("bondi", "on").await;
    assert_eq!(f.status, "failed");
    assert!(f.reason.unwrap().contains("ended failed"));
    let notices = fx.notices().await;
    assert_eq!(notices.len(), 2);
    assert!(
        notices
            .iter()
            .all(|n| n.body.as_deref().unwrap().contains("due 2026-03-02")),
        "a notice names the flip's due date: {notices:?}"
    );
}
