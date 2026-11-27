//! Which production a production attempt's baseline describes: only the build
//! production served right before the move being checked.

use super::*;
use crate::promote_rollback::{fixture_builds, pointers, rollback};

/// The checker claims the newest production attempt of `build`: the baseline
/// its claim hands the runner.
async fn claimed_baseline(agent: &Agent, build: Uuid) -> Value {
    let base = checks(agent, build);
    let claim = Some(claim_body("production"));
    let (status, claimed) = checker("POST", &format!("{base}/claim"), claim).await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    assert_eq!(claimed["open"], json!(["production_smoke"]), "{claimed}");
    claimed["baseline"].clone()
}

/// `agent-prod` is live. B's staging comparison sees it; then another build is
/// promoted; then B. B's comparison describes a production that was replaced
/// before B's move, so B's production attempt gets no baseline. Rolled back
/// to the build in between, that build's own comparison (against
/// `agent-prod`) does not describe B either: no baseline, so nothing it
/// always showed can be called new.
#[tokio::test]
async fn a_comparison_with_a_production_that_was_replaced_is_no_baseline() {
    let agent = gated().await;
    let (first_live, _) = fixture_builds(&agent).await;
    let b = draft(&agent, "bl-b", None).await;
    pass_staging(&agent, b, json!(["TypeError: old on agent-prod"])).await;
    let between = draft(&agent, "bl-between", None).await;
    pass_staging(&agent, between, json!([])).await;
    promote(&agent).await;
    assert_eq!(pointers(&agent).await.0, Some(between));
    // The build in between replaced `agent-prod`, which its comparison saw.
    let baseline = claimed_baseline(&agent, between).await;
    assert_eq!(baseline["screens"][0]["path"], "stores", "{baseline}");

    let (status, plan) = crate::promote_rollback::promote(
        &agent,
        "production",
        json!({ "from": "staging", "build": b }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(pointers(&agent).await.0, Some(b));
    assert_ne!(Some(first_live), pointers(&agent).await.0);
    assert_eq!(
        claimed_baseline(&agent, b).await,
        json!(null),
        "B was compared with agent-prod, and replaced another build"
    );

    let (status, rolled) = rollback(&agent, "production", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{rolled}");
    assert_eq!(pointers(&agent).await.0, Some(between));
    assert_eq!(
        claimed_baseline(&agent, between).await,
        json!(null),
        "the rollback replaced B, which nothing compared this build with"
    );
}
