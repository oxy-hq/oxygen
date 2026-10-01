//! The coordinator run feed (`list_runs_filtered`, `list_active_runs`) never
//! shows a workspace preview's dry run to the customer.
//!
//! A held preview procedure run is saved as an ordinary `workflow` run with
//! `metadata.trigger = "preview"`; it is a staffer running an unmerged branch,
//! so it must stay out of the feed even with `include_system` (the customer's
//! own toggle).
//!
//! Run:
//!   cargo nextest run -p agentic-runtime --test integration -E 'test(run_feed_test)'

use agentic_runtime::crud;
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

use crate::integration_tests::test_db;

struct Seeded {
    workspace: Uuid,
    customer: String,
    unlabelled: String,
    preview: String,
}

/// Three root `workflow` runs in a fresh workspace, all still running: the
/// customer's own, one with no metadata at all, and a preview dry run.
async fn seed(db: &DatabaseConnection) -> Seeded {
    let workspace = Uuid::new_v4();
    let customer = format!("feed-{}", Uuid::new_v4());
    let unlabelled = format!("feed-{}", Uuid::new_v4());
    let preview = format!("feed-{}", Uuid::new_v4());
    let rows = [
        (&customer, Some(json!({ "trigger": "schedule" }))),
        (&unlabelled, None),
        (
            &preview,
            Some(json!({ "trigger": "preview", "workflow_ref": "preview:x" })),
        ),
    ];
    for (id, metadata) in rows {
        crud::insert_run(db, id, "q", None, "workflow", metadata, workspace)
            .await
            .expect("insert_run");
    }
    Seeded {
        workspace,
        customer,
        unlabelled,
        preview,
    }
}

fn ids(runs: Vec<agentic_runtime::entity::run::Model>) -> Vec<String> {
    let mut ids: Vec<String> = runs.into_iter().map(|r| r.id).collect();
    ids.sort();
    ids
}

fn expected(s: &Seeded) -> Vec<String> {
    let mut want = vec![s.customer.clone(), s.unlabelled.clone()];
    want.sort();
    want
}

#[tokio::test]
async fn list_runs_filtered_hides_preview_runs() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for include_system in [false, true] {
        let (runs, total) =
            crud::list_runs_filtered(&db, s.workspace, None, None, None, include_system, 0, 50)
                .await
                .expect("list_runs_filtered");
        assert_eq!(total, 2, "include_system={include_system}");
        let got = ids(runs);
        assert!(!got.contains(&s.preview), "preview run leaked: {got:?}");
        assert_eq!(got, expected(&s), "include_system={include_system}");
    }
}

#[tokio::test]
async fn list_active_runs_hides_preview_runs() {
    let Some(db) = test_db().await else { return };
    let s = seed(&db).await;
    for include_system in [false, true] {
        let runs = crud::list_active_runs(&db, s.workspace, include_system)
            .await
            .expect("list_active_runs");
        let got = ids(runs);
        assert!(!got.contains(&s.preview), "preview run leaked: {got:?}");
        assert_eq!(got, expected(&s), "include_system={include_system}");
    }
}
