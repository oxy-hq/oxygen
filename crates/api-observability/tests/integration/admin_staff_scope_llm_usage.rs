//! The staff console's LLM cost (`/admin/metrics/llm-usage`) under a bounded
//! grant. Moved from `oxy-app`'s `tests/platform/admin_staff_scope/directories.rs`
//! with the handlers (admin split, PR 1); the fixture is that suite's, included
//! by path so the two cannot drift.

use axum::extract::Query;
use axum::http::StatusCode;
use oxy_api_observability::admin::metrics::{UsageQuery, llm_usage};
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

#[path = "../../../test-utils/harness/admin_staff_scope_fixture.rs"]
#[allow(dead_code)]
pub(crate) mod fixture;
use fixture::{as_actor, reply, world};

/// One run with one LLM call, in `workspace`.
async fn llm_run(db: &DatabaseConnection, workspace: Uuid) {
    let id = format!("run-{}", Uuid::new_v4());
    agentic_runtime::crud::runs::insert_run(db, &id, "q", None, "analytics", None, workspace)
        .await
        .expect("seed run");
    let events = [
        ("llm_start", json!({ "prompt_tokens": 100 })),
        (
            "llm_end",
            json!({ "model": "staff-scope-model", "output_tokens": 10 }),
        ),
    ];
    for (seq, (kind, payload)) in events.iter().enumerate() {
        agentic_runtime::crud::events::insert_event(db, &id, seq as i64, kind, payload, 0)
            .await
            .expect("seed run event");
    }
}

/// Token totals and the per-org leaderboard are every tenant's spend.
#[tokio::test]
async fn a_bounded_grants_llm_usage_counts_only_its_own_orgs_runs() {
    let w = world().await;
    llm_run(&w.db, w.ws_a).await;
    llm_run(&w.db, w.ws_b).await;
    llm_run(&w.db, w.ws_b).await;
    llm_run(&w.db, w.ws_orphan).await;
    let query = || Query(UsageQuery::default());

    let usage = reply(llm_usage(as_actor(&w.bounded), query()).await).await;
    assert_eq!(usage.status, StatusCode::OK);
    assert_eq!(
        usage.body["total"]["run_count"], 1,
        "{}",
        usage.body["total"]
    );
    assert_eq!(usage.body["total"]["input_tokens"], 100);
    assert_eq!(
        usage.column(Some("by_org"), "org_id"),
        vec![w.org_a.to_string()],
        "org B's LLM spend is on a bounded grant's leaderboard"
    );

    for (who, actor) in w.everything_readers() {
        let usage = reply(llm_usage(as_actor(actor), query()).await).await;
        assert_eq!(usage.body["total"]["run_count"], 4, "{who}");
        assert_eq!(usage.column(Some("by_org"), "org_id").len(), 2, "{who}");
    }
}
