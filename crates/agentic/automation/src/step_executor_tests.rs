//! The preview review hooks on the step path: `review_sql` for `execute_sql`,
//! `review_http` for `http_request`.
//!
//! Every host but the preview platform keeps the trait defaults, so the first
//! test here is the one production depends on: a host that overrides nothing
//! runs exactly what it ran before the hooks existed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::run_automation_step;
use crate::review::{HttpReview, SqlReview};
use crate::workspace::{IntegrationConfig, WorkspaceContext};

type Events = Arc<Mutex<Vec<String>>>;

/// Records every statement that reaches it.
struct Recorder {
    events: Events,
}

#[async_trait]
impl DatabaseConnector for Recorder {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::Postgres
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.events.lock().unwrap().push(format!("execute:{sql}"));
        Ok(ExecutionResult::empty())
    }
}

/// Implements `WorkspaceContext` for a fake host: the parts every fake shares,
/// plus whatever overrides follow. The whole `#[async_trait]` impl is
/// generated, so the attribute sees every `async fn` in it (a macro invoked
/// *inside* an `#[async_trait]` impl expands after the attribute has run).
macro_rules! fake_host {
    ($host:ident, $($overrides:tt)*) => {
        #[async_trait]
        impl WorkspaceContext for $host {
            fn workspace_path(&self) -> Option<&Path> {
                None
            }
            fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
                vec![]
            }
            async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
                Ok(vec![])
            }
            async fn resolve_automation_yaml(
                &self,
                _: &str,
            ) -> Result<String, crate::WorkspaceReadError> {
                unreachable!("not exercised")
            }
            async fn get_integration(&self, _: &str) -> Result<IntegrationConfig, String> {
                unreachable!("not exercised")
            }
            async fn get_connector(
                &self,
                name: &str,
            ) -> Result<Arc<dyn DatabaseConnector>, String> {
                self.events.lock().unwrap().push(format!("connector:{name}"));
                Ok(Arc::new(Recorder {
                    events: self.events.clone(),
                }))
            }
            $($overrides)*
        }
    };
}

/// Overrides no review method: production's host.
struct PlainHost {
    events: Events,
}

fake_host! { PlainHost, }

/// Answers every review with a fixed verdict and records what it was asked.
struct ReviewingHost {
    events: Events,
    sql: Result<SqlReview, String>,
    http: HttpReview,
}

// Brace-delimited so rustfmt leaves the item list alone: with parentheses it
// reads the items as call arguments and inserts a comma between them.
fake_host! {
    ReviewingHost,
    async fn review_sql(&self, database: &str, sql: &str) -> Result<SqlReview, String> {
        self.events
            .lock()
            .unwrap()
            .push(format!("review:{database}:{sql}"));
        self.sql.clone()
    }
    async fn review_http(&self, method: &str, url: &str) -> HttpReview {
        self.events
            .lock()
            .unwrap()
            .push(format!("review_http:{method} {url}"));
        self.http.clone()
    }
}

fn events() -> Events {
    Arc::new(Mutex::new(Vec::new()))
}

fn reviewing(sql: Result<SqlReview, String>) -> (ReviewingHost, Events) {
    let e = events();
    let host = ReviewingHost {
        events: e.clone(),
        sql,
        http: HttpReview::Proceed,
    };
    (host, e)
}

fn logged(events: &Events) -> Vec<String> {
    events.lock().unwrap().clone()
}

async fn run_sql_step(host: &dyn WorkspaceContext, sql: &str, ctx: Value) -> Result<Value, String> {
    let step =
        json!({ "name": "s", "type": "execute_sql", "database": "warehouse", "sql_query": sql });
    let out = run_automation_step(host, step, ctx, json!({})).await?;
    Ok(serde_json::from_str(&out).expect("step output is JSON"))
}

#[tokio::test]
async fn default_review_changes_nothing() {
    let e = events();
    let host = PlainHost { events: e.clone() };

    // The defaults themselves.
    assert_eq!(
        host.review_sql("warehouse", "DELETE FROM t").await,
        Ok(SqlReview::Proceed)
    );
    assert_eq!(
        host.review_http("POST", "https://example.com").await,
        HttpReview::Proceed
    );

    // And the step: the rendered SQL reaches the connector unchanged, and the
    // result has no preview note — the shape it had before the hook existed.
    let out = run_sql_step(&host, "DELETE FROM t WHERE n = {{ n }}", json!({ "n": 7 }))
        .await
        .expect("step runs");
    assert_eq!(
        out,
        json!({ "columns": [], "rows": [], "row_count": 0, "truncated": false,
                "sql": "DELETE FROM t WHERE n = 7" })
    );
    assert_eq!(
        logged(&e),
        vec!["connector:warehouse", "execute:DELETE FROM t WHERE n = 7"]
    );
}

#[tokio::test]
async fn a_held_step_succeeds_with_the_hold_recorded() {
    let (host, e) = reviewing(Ok(SqlReview::Hold {
        reason: "`warehouse` cannot be written in a workspace preview".into(),
        verb: "DELETE".into(),
        targets: vec!["public.orders".into()],
    }));
    let out = run_sql_step(
        &host,
        "DELETE FROM public.orders WHERE business_date = '{{ date }}'",
        json!({ "date": "2026-09-27" }),
    )
    .await
    .expect("a held step succeeds, so the procedure goes on");

    assert_eq!(
        out,
        json!({
            "columns": [], "rows": [], "row_count": 0, "truncated": false,
            "sql": "DELETE FROM public.orders WHERE business_date = '2026-09-27'",
            "preview": {
                "held": true,
                "reason": "`warehouse` cannot be written in a workspace preview",
                "verb": "DELETE",
                "targets": ["public.orders"],
            },
        })
    );
    assert_eq!(
        logged(&e),
        vec!["review:warehouse:DELETE FROM public.orders WHERE business_date = '2026-09-27'"],
        "no connector was even built"
    );
}

/// The review sees what would run — the rendered body with the task's
/// `variables` merged in — and it is asked before any connector exists.
#[tokio::test]
async fn review_runs_after_render() {
    let (host, e) = reviewing(Ok(SqlReview::Proceed));
    let step = json!({
        "name": "s", "type": "execute_sql", "database": "warehouse",
        "sql_query": "SELECT * FROM {{ table }} WHERE d = '{{ day }}'",
        "variables": { "table": "rollups.{{ grain }}" },
    });
    run_automation_step(
        &host,
        step,
        json!({ "grain": "daily", "day": "2026-09-27" }),
        json!({}),
    )
    .await
    .expect("step runs");
    let rendered = "SELECT * FROM rollups.daily WHERE d = '2026-09-27'";
    assert_eq!(
        logged(&e),
        vec![
            format!("review:warehouse:{rendered}"),
            "connector:warehouse".to_string(),
            format!("execute:{rendered}"),
        ]
    );
}

#[tokio::test]
async fn a_rewrite_runs_the_rewritten_sql_and_attaches_notes() {
    let notes = json!({ "redirected_writes": ["preview_k__toast_pos.t"] });
    let (host, e) = reviewing(Ok(SqlReview::Rewrite {
        sql: "INSERT INTO preview_k__toast_pos.t SELECT 1".into(),
        notes: notes.clone(),
    }));
    let out = run_sql_step(&host, "INSERT INTO toast_pos.t SELECT 1", json!({}))
        .await
        .expect("step runs");
    assert_eq!(out["sql"], "INSERT INTO preview_k__toast_pos.t SELECT 1");
    assert_eq!(out["preview"], notes);
    assert_eq!(
        logged(&e).last().map(String::as_str),
        Some("execute:INSERT INTO preview_k__toast_pos.t SELECT 1")
    );
}

/// A host that cannot decide fails the step; it never falls back to running.
#[tokio::test]
async fn a_review_error_fails_the_step_without_running_it() {
    let (host, e) = reviewing(Err("preview registry unavailable".into()));
    let err = run_sql_step(&host, "DELETE FROM t", json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("preview registry unavailable"), "{err}");
    assert!(
        logged(&e).iter().all(|l| l.starts_with("review:")),
        "{:?}",
        logged(&e)
    );
}

#[tokio::test]
async fn a_held_http_request_is_not_sent() {
    let e = events();
    let host = ReviewingHost {
        events: e.clone(),
        sql: Ok(SqlReview::Proceed),
        http: HttpReview::Hold {
            reason: "only GET and HEAD run in a workspace preview".into(),
        },
    };
    let step = json!({
        "name": "notify", "type": "http_request", "method": "post",
        "url": "https://hooks.example.invalid/notify?day={{ day }}",
        "body": "{}",
    });
    // Sent for real, this would fail: the host does not resolve. `Ok` is the
    // proof that nothing left the process.
    let out = run_automation_step(&host, step, json!({ "day": "2026-09-27" }), json!({}))
        .await
        .expect("a held request succeeds without sending");
    let out: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["preview"]["held"], true);
    assert_eq!(out["preview"]["method"], "POST");
    assert_eq!(
        out["preview"]["url"], "https://hooks.example.invalid/notify?day={{ day }}",
        "the unrendered template is recorded, so a secret in the URL is not"
    );
    assert_eq!(
        logged(&e),
        vec!["review_http:POST https://hooks.example.invalid/notify?day=2026-09-27"]
    );
}

/// D5 for `http_request`: a pod that does not know previews, handed a step
/// whose method a preview scoped, fails before it builds the request — its
/// production `review_http` says Proceed, so the method itself is the fence.
/// `persist_to_secret` is never reached, and its scoped name is not one the
/// secret store accepts either.
#[tokio::test]
async fn a_scoped_method_is_refused_before_anything_is_sent() {
    let e = events();
    let host = PlainHost { events: e.clone() };
    let run = "3f0b6c1e-8d2a-4c61-9d7e-0a1b2c3d4e5f";
    let step = json!({
        "name": "rotate", "type": "http_request",
        "method": crate::preview_names::scoped(run, "POST"),
        "url": "https://oauth.example.invalid/token",
        "persist_to_secret": { "from": "/refresh_token",
                               "name": crate::preview_names::scoped(run, "QB_REFRESH_TOKEN") },
    });
    let err = run_automation_step(&host, step, json!({}), json!({}))
        .await
        .expect_err("a scoped method cannot be sent");
    assert!(
        err.contains("invalid method"),
        "refused at the method, not the send: {err}"
    );
    assert!(
        logged(&e).is_empty(),
        "no connector, no review record: {:?}",
        logged(&e)
    );
}

/// The new pod's hold names the verb, not the scoped form.
#[tokio::test]
async fn a_held_scoped_method_is_reported_as_its_verb() {
    let host = ReviewingHost {
        events: events(),
        sql: Ok(SqlReview::Proceed),
        http: HttpReview::Hold {
            reason: "held".into(),
        },
    };
    let step = json!({
        "name": "notify", "type": "http_request",
        "method": crate::preview_names::scoped("3f0b6c1e-8d2a-4c61-9d7e-0a1b2c3d4e5f", "DELETE"),
        "url": "https://hooks.example.invalid/x",
    });
    let out = run_automation_step(&host, step, json!({}), json!({}))
        .await
        .unwrap();
    let out: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["preview"]["method"], "DELETE");
}
