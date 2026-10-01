//! Previews end to end: the whole staff journey through one router assembled
//! the way production mounts it, with nothing seeded that the product would
//! have produced itself. The branch is compiled by the real compile worker
//! from a real git repository, its change check is run by the worker the
//! compile queued it for, and a dry run is driven the same way — so a break
//! anywhere between "staff click Preview" and "the run says what it held"
//! fails here, where the per-route suites each stop at their own seam.
//!
//! Database-backed (`Schema::All`), one database per test. The warehouse is a
//! recording ClickHouse stand-in and HTTPS egress goes through a recording
//! proxy, so what a dry run did NOT send is asserted, not assumed.

mod fakes;
mod fixture;
mod read_only_exec;
mod worker;

use axum::http::StatusCode;
use sea_orm::EntityTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use fakes::FakeEgress;
use fixture::{
    BRANCH, BRANCH_Q, Fx, HOOK, PROCEDURE, WRITE_SQL, call, enable_runs, eventually, names,
    previews, ready_preview, revision_of, setup, workspace,
};

fn databases(fx: &Fx) -> String {
    format!("/{}/databases", fx.ws)
}

/// Create → compiled → listed ready with its checks → a page answers from the
/// branch and says so → a write under the preview is refused before it runs →
/// delete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staff_preview_a_branch_from_create_to_delete() {
    let fx = setup().await;
    let _worker = worker::start(&fx).await;

    let staging = compiled_and_checked(&fx, &ready_preview(&fx).await).await;
    the_page_reads_the_branch_and_says_so(&fx, staging).await;
    a_write_is_refused_before_it_runs(&fx, staging).await;

    let gone = call(
        &fx.staff,
        "DELETE",
        format!("{}?branch={BRANCH_Q}", previews(&fx)),
        None,
        None,
    )
    .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.body);
    let list = call(&fx.staff, "GET", previews(&fx), None, None).await;
    assert_eq!(list.body["items"], json!([]), "{}", list.body);
}

/// The listed item is the branch head, compiled as a staging revision that was
/// never promoted, and its change check is done. The staging revision.
async fn compiled_and_checked(fx: &Fx, item: &Value) -> Uuid {
    assert_eq!(item["branch"], BRANCH);
    assert_eq!(item["sha"], fx.repo.feat_sha.as_str());
    assert_eq!(item["error"], Value::Null);
    let staging = revision_of(item);
    assert_ne!(staging, fx.main_revision);
    let rev = entity::revisions::Entity::find_by_id(staging)
        .one(&fx.db)
        .await
        .unwrap()
        .expect("the compiled revision");
    assert_eq!(
        (rev.kind.as_str(), rev.status.as_str(), rev.git_sha.as_str()),
        ("staging", "ready", fx.repo.feat_sha.as_str())
    );
    assert_eq!(
        workspace(&fx.db, fx.ws).await.current_revision_id,
        Some(fx.main_revision),
        "a preview never promotes"
    );
    // The branch adds one automation and no pipeline.
    assert_eq!(
        item["checks"],
        json!({ "status": "done", "needs_reset": 0, "warnings": 0, "transforms": 1 })
    );
    let checks = call(
        &fx.staff,
        "GET",
        format!("{}/checks?branch={BRANCH_Q}", previews(fx)),
        None,
        None,
    )
    .await;
    assert_eq!(checks.status, StatusCode::OK, "{}", checks.body);
    assert_eq!(checks.body["revision_id"], staging.to_string());
    assert_eq!(checks.body["pipelines"], json!([]));
    assert_eq!(checks.body["transforms"][0]["file_path"], PROCEDURE);
    staging
}

/// Live without the header; the branch with it, stamped `x-oxy-preview`.
async fn the_page_reads_the_branch_and_says_so(fx: &Fx, staging: Uuid) {
    let live = call(&fx.staff, "GET", databases(fx), None, None).await;
    assert_eq!(names(&live), ["warehouse"]);
    assert_eq!(live.preview, None);
    let pinned = call(&fx.staff, "GET", databases(fx), None, Some(staging)).await;
    assert_eq!(names(&pinned), ["warehouse", "branch_warehouse"]);
    assert_eq!(
        pinned.preview.as_deref(),
        Some(format!("{BRANCH}@{staging}").as_str())
    );
}

/// `409 preview_read_only`, stamped, and the working copy untouched.
async fn a_write_is_refused_before_it_runs(fx: &Fx, staging: Uuid) {
    let config = fx.repo.root.join("config.yml");
    let before = std::fs::read_to_string(&config).unwrap();
    let added = json!({ "warehouses": [{ "name": "added", "type": "clickhouse" }] });
    let write = call(&fx.staff, "POST", databases(fx), Some(added), Some(staging)).await;
    assert_eq!(write.status, StatusCode::CONFLICT, "{}", write.body);
    assert_eq!(write.body["code"], "preview_read_only");
    assert_eq!(
        write.body["message"],
        "Adding a database isn't available in a preview; merge the branch to run it"
    );
    assert!(
        write.preview.is_some(),
        "a refusal is a preview response too"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
}

/// A dry run of the branch's procedure: its read reaches the warehouse, its
/// warehouse write and its POST are held — reported with verb and targets —
/// and neither reaches anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dry_run_reads_live_and_holds_the_write_and_the_post() {
    let fx = setup().await;
    // The environment is set before the worker starts and put back when the
    // test ends (`fixture::EnvGuard`).
    let egress = FakeEgress::start().await;
    let _proxy = egress.route_https_egress_here();
    let _runs = enable_runs(true);
    let _worker = worker::start(&fx).await;
    let staging = revision_of(&ready_preview(&fx).await);

    let detail = dry_run_to_the_end(&fx).await;
    nothing_but_the_read_left(&fx, &egress);
    the_run_reports_each_hold(&detail, staging);

    // The control: a POST that IS sent to that URL is seen by the proxy.
    let sent_anyway = reqwest::Client::new().post(HOOK).body("{}").send().await;
    assert!(sent_anyway.is_err(), "the proxy refuses every tunnel");
    assert!(
        egress.tunnels().iter().any(|t| t.starts_with(HOOK_TUNNEL)),
        "the proxy must see a POST that is sent: {:?}",
        egress.tunnels()
    );
}

/// The request line a POST to [`HOOK`] opens its tunnel with.
const HOOK_TUNNEL: &str = "CONNECT hooks.example.test:443";

/// Submit the procedure as staff; the run once the worker finished it.
async fn dry_run_to_the_end(fx: &Fx) -> Value {
    let body = json!({ "branch": BRANCH, "kind": "procedure", "ref": PROCEDURE });
    let submitted = call(
        &fx.staff,
        "POST",
        format!("{}/runs", previews(fx)),
        Some(body),
        None,
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED, "{}", submitted.body);
    assert_eq!(
        submitted.body["state"], "running",
        "nothing else in progress"
    );
    let run_id = submitted.body["run_id"].as_str().unwrap().to_string();
    eventually(fx, format!("{}/runs/{run_id}", previews(fx)), |d| {
        d["state"] == "finished"
    })
    .await
}

/// The read went to the warehouse, the write did not, and no tunnel was ever
/// asked for the webhook's host.
fn nothing_but_the_read_left(fx: &Fx, egress: &FakeEgress) {
    let sent = fx.warehouse.statements();
    assert!(
        sent.iter().any(|s| s.contains("analytics.orders")),
        "the read ran against the warehouse: {sent:?}"
    );
    assert!(
        !sent.iter().any(|s| s.contains("journal")),
        "a held write reached the warehouse: {sent:?}"
    );
    assert!(
        !egress.tunnels().iter().any(|t| t.starts_with(HOOK_TUNNEL)),
        "a held POST was sent: {:?}",
        egress.tunnels()
    );
}

fn the_run_reports_each_hold(detail: &Value, staging: Uuid) {
    assert_eq!(detail["outcome"], "succeeded", "{detail}");
    assert_eq!(detail["revision_id"], staging.to_string());
    assert_eq!(detail["target_ref"], PROCEDURE);
    assert_eq!(detail["held_count"], 2, "{detail}");
    let steps = detail["steps"].as_array().expect("steps");
    let step = |name: &str| {
        steps
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no step {name}: {detail}"))
    };
    let read = step("load_orders");
    assert_eq!(
        (read["kind"].as_str(), read["status"].as_str()),
        (Some("execute_sql"), Some("succeeded"))
    );
    assert_eq!(read["held"], Value::Null);
    let write = step("post_journal");
    assert_eq!(write["status"], "held", "{write}");
    assert_eq!(write["held"]["verb"], "INSERT");
    assert_eq!(write["held"]["targets"], json!(["analytics.journal"]));
    assert_eq!(write["held"]["sql"], WRITE_SQL);
    let post = step("notify");
    assert_eq!(
        (post["kind"].as_str(), post["status"].as_str()),
        (Some("http_request"), Some("held"))
    );
    assert_eq!(post["held"]["verb"], "POST");
    assert_eq!(post["held"]["targets"], json!([HOOK]));
    assert_eq!(post["held"]["sql"], Value::Null);
}

/// Staff only, flag-gated, and a preview header is honoured for staff alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn previews_are_staff_only_and_a_customer_header_reads_live() {
    let fx = setup().await;

    // Flag off: staff get the flag's 404 on every runs route; an org admin
    // still gets the guard's 403 (it runs first). Asked before the worker
    // starts, because the flag is an environment variable and is only changed
    // while nothing else reads the environment (`fixture::EnvGuard`); the flag
    // answers before any preview is looked up, so none is needed yet.
    {
        let _off = enable_runs(false);
        for (method, uri, body) in run_routes(&fx) {
            let r = call(&fx.staff, method, uri.clone(), body.clone(), None).await;
            assert_eq!(
                r.status,
                StatusCode::NOT_FOUND,
                "{method} {uri}: {}",
                r.body
            );
            assert_eq!(r.body["code"], "preview_runs_disabled", "{method} {uri}");
            let r = call(&fx.customer, method, uri.clone(), body, None).await;
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
    }

    let _on = enable_runs(true);
    let _worker = worker::start(&fx).await;
    let staging = revision_of(&ready_preview(&fx).await);

    // The header from an org admin: served live, never stamped.
    let page = call(&fx.customer, "GET", databases(&fx), None, Some(staging)).await;
    assert_eq!(names(&page), ["warehouse"]);
    assert_eq!(page.preview, None);
    let page = call(&fx.staff, "GET", databases(&fx), None, Some(staging)).await;
    assert!(page.preview.is_some(), "control: staff are pinned");

    // Flag on: an org admin gets 403 on every previews route; staff pass the
    // guard on each (the control), the delete last.
    for (method, uri, body) in every_route(&fx) {
        let r = call(&fx.customer, method, uri.clone(), body.clone(), None).await;
        assert_eq!(
            r.status,
            StatusCode::FORBIDDEN,
            "{method} {uri}: {}",
            r.body
        );
        let r = call(&fx.staff, method, uri.clone(), body, None).await;
        assert_ne!(
            r.status,
            StatusCode::FORBIDDEN,
            "{method} {uri}: {}",
            r.body
        );
    }
}

type Route = (&'static str, String, Option<Value>);

/// Every previews route; the delete last, so the staff control of the others
/// still has a preview to act on.
fn every_route(fx: &Fx) -> Vec<Route> {
    let b = previews(fx);
    let source = json!({ "pipeline": "qb", "environment": "sandbox",
                         "overrides": { "realm_id": "1", "access_token_var": "QB_SANDBOX_TOKEN" } });
    let mut routes = vec![
        ("GET", b.clone(), None),
        ("POST", b.clone(), Some(json!({ "branch": BRANCH }))),
        ("POST", format!("{b}/refresh?branch={BRANCH_Q}"), None),
        ("GET", format!("{b}/checks?branch={BRANCH_Q}"), None),
        ("GET", format!("{b}/sources"), None),
        ("PUT", format!("{b}/sources"), Some(source)),
    ];
    routes.extend(run_routes(fx));
    routes.push(("DELETE", format!("{b}?branch={BRANCH_Q}"), None));
    routes
}

/// The runs routes, with a ref no revision has so a staff call starts nothing.
fn run_routes(fx: &Fx) -> Vec<Route> {
    let b = previews(fx);
    let missing =
        json!({ "branch": BRANCH, "kind": "procedure", "ref": "workflows/nope.procedure.yml" });
    vec![
        ("POST", format!("{b}/runs"), Some(missing)),
        ("GET", format!("{b}/runs?branch={BRANCH_Q}"), None),
        ("GET", format!("{b}/runs/{}", Uuid::new_v4()), None),
    ]
}
