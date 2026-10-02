//! A check run in a named app environment
//! (`POST …/functions/{name}/runs?environment=<name>`), end to end.
//!
//! The app has a production build and a **different** staging build, so "which
//! build ran" is observable in the answer. The run is queued by the admin route
//! and executed by the production driver entry point and executor, as in the
//! parent module. Every refusal around a check run is `environment_refusals`'.
//!
//! - a `check: true` function runs in staging: the staging build, the
//!   non-production policy (a write is held and listed, `ctx.channel` is
//!   `staging`, a secret is the environment's), and the run reports its
//!   `environment` and `invocation_id`;
//! - without the parameter the same check runs in production, as before;
//! - the functions list is the named environment's build's.

use axum::http::StatusCode;
use oxy::service::secret_manager::SecretManagerService;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{get_admin, platform, post_admin, spawn_driver, wait_for_run};
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, invocations, publish_build, seeded_tenant,
};
use crate::staging_function_homes::publish_env_build;
use crate::staging_functions::make_guest_staff;

pub(super) const APP: &str = "fn-env-checks";
pub(super) const PRODUCTION_BUILD: &str = "checks-prod-1";
pub(super) const STAGING_BUILD: &str = "checks-stg-1";

const SMOKE_PRODUCTION_JS: &str = r#"
export default async (req, ctx) =>
  Response.json({ build: "production-code", channel: ctx.channel });
"#;
/// Staging's `smoke` attempts a third-party write, which the non-production
/// policy holds (answers `409`, unsent) and lists.
const SMOKE_STAGING_JS: &str = r#"
export default async (req, ctx) => {
  const held = await ctx.fetch("https://api.example.com/orders", { method: "POST", body: "{}" });
  return Response.json({ build: "staging-code", channel: ctx.channel, write: held.status });
};
"#;
const PLAIN_JS: &str = "export default async () => Response.json({ ran: true });";

/// Production's build: `smoke` and `was-a-check` are checks, `plain` is not.
fn production_functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "smoke",
            manifest: json!({ "check": true }),
            js: SMOKE_PRODUCTION_JS,
        },
        FunctionSpec {
            name: "was-a-check",
            manifest: json!({ "check": true }),
            js: PLAIN_JS,
        },
        FunctionSpec {
            name: "plain",
            manifest: json!({ "route": true }),
            js: PLAIN_JS,
        },
    ]
}

/// Staging's build: `smoke` is still a check; `was-a-check` no longer is.
fn staging_functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "smoke",
            manifest: json!({ "check": true }),
            js: SMOKE_STAGING_JS,
        },
        FunctionSpec {
            name: "was-a-check",
            manifest: json!({ "route": true }),
            js: PLAIN_JS,
        },
        FunctionSpec {
            name: "plain",
            manifest: json!({ "route": true }),
            js: PLAIN_JS,
        },
    ]
}

/// The app with a promoted build and a newer, different staging build.
/// Returns the app id; the guest is Oxy staff from here on.
pub(super) async fn two_builds(t: &Tenant) -> Uuid {
    let ws = demo_workspace_id();
    let app_id = publish_build(t, APP, ws, PRODUCTION_BUILD, true, &production_functions())
        .await
        .app_id;
    publish_build(t, APP, ws, STAGING_BUILD, false, &staging_functions()).await;
    make_guest_staff();
    app_id
}

pub(super) async fn build_pk(t: &Tenant, app_id: Uuid, label: &str) -> Uuid {
    use entity::app_builds;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .filter(app_builds::Column::BuildId.eq(label))
        .one(&t.db)
        .await
        .expect("read builds")
        .unwrap_or_else(|| panic!("build {label}"))
        .id
}

#[tokio::test]
async fn a_check_runs_in_staging_on_the_staging_build_under_the_staging_policy() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(t.db.clone(), platform);

    let (status, queued) = post_admin(&format!(
        "/apps/{app_id}/functions/smoke/runs?environment=staging"
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "run in staging: {queued}");
    assert_eq!(queued["environment"], "staging");
    let staging_run = queued["run_id"].as_str().expect("a run id").to_string();

    // Without the parameter the same route queues production's run, as before.
    let (status, queued) = post_admin(&format!("/apps/{app_id}/functions/smoke/runs")).await;
    assert_eq!(status, StatusCode::OK, "run now: {queued}");
    assert_eq!(queued["environment"], "production");
    let production_run = queued["run_id"].as_str().expect("a run id").to_string();

    let staging = wait_for_run(app_id, &staging_run).await;
    let production = wait_for_run(app_id, &production_run).await;
    driver.abort();

    assert_eq!(staging["status"], "done", "staging run: {staging}");
    assert_eq!(staging["environment"], "staging");
    assert_eq!(staging["trigger"], "manual");
    let answer_of = |run: &Value| -> Value {
        serde_json::from_str(run["answer"].as_str().expect("an answer")).expect("JSON")
    };
    assert_eq!(
        answer_of(&staging),
        json!({ "build": "staging-code", "channel": "staging", "write": 409 }),
        "the staging build ran, as staging, and its write was held"
    );
    assert_eq!(production["status"], "done", "production run: {production}");
    assert_eq!(production["environment"], "production");
    assert_eq!(
        answer_of(&production),
        json!({ "build": "production-code", "channel": "production" }),
        "without the parameter the run is production's"
    );

    // One invocation each, recorded against the environment and build that ran
    // it; the run names its own.
    let rows = invocations(&t.db, app_id, "smoke").await;
    let by_environment = |name: &str| {
        let mut matching = rows.iter().filter(|r| r.environment == name);
        let row = matching
            .next()
            .unwrap_or_else(|| panic!("an invocation in {name}"));
        assert!(matching.next().is_none(), "exactly one in {name}");
        row
    };
    let (staged, live) = (by_environment("staging"), by_environment("production"));
    assert_eq!(staged.build_id, build_pk(&t, app_id, STAGING_BUILD).await);
    assert_eq!(live.build_id, build_pk(&t, app_id, PRODUCTION_BUILD).await);
    for row in [staged, live] {
        // What a production check run records: the manual trigger, no caller.
        assert_eq!(
            (row.mode.as_str(), row.status.as_str(), row.user_id),
            ("manual", "success", None)
        );
    }
    assert_eq!(staging["invocation_id"], staged.id.to_string());
    assert_eq!(production["invocation_id"], live.id.to_string());

    // The non-production policy held the write, and the read-back lists it.
    let (status, held) = get_admin(&format!("/apps/{app_id}/invocations/{}/held", staged.id)).await;
    assert_eq!(status, StatusCode::OK, "held: {held}");
    assert_eq!(held["environment"], "staging");
    assert_eq!(held["function"], "smoke");
    assert_eq!(held["build_id"], STAGING_BUILD);
    let ops: Vec<&str> = held["held"]
        .as_array()
        .expect("held")
        .iter()
        .filter_map(|w| w["op"].as_str())
        .collect();
    assert_eq!(ops, vec!["fetch"], "held: {held}");
    let (_, held) = get_admin(&format!("/apps/{app_id}/invocations/{}/held", live.id)).await;
    assert_eq!(held["held"], json!([]), "production holds nothing: {held}");
}

/// Staff see, per environment, the functions of the build that environment
/// serves — which is what a check run there executes.
#[tokio::test]
async fn the_functions_list_is_the_named_environments_build() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t).await;
    let checks = |listed: &Value| -> Vec<(String, bool)> {
        listed
            .as_array()
            .expect("a list")
            .iter()
            .map(|f| {
                (
                    f["name"].as_str().expect("name").to_string(),
                    f["check"].as_bool().expect("check"),
                )
            })
            .collect()
    };
    let expect = |was_a_check: bool| {
        vec![
            ("plain".to_string(), false),
            ("smoke".to_string(), true),
            ("was-a-check".to_string(), was_a_check),
        ]
    };

    let (status, live) = get_admin(&format!("/apps/{app_id}/functions")).await;
    assert_eq!(status, StatusCode::OK, "{live}");
    assert_eq!(checks(&live), expect(true), "absent: the live build");
    let (_, staged) = get_admin(&format!("/apps/{app_id}/functions?environment=staging")).await;
    assert_eq!(checks(&staged), expect(false), "staging's build");
    let (_, named) = get_admin(&format!("/apps/{app_id}/functions?environment=production")).await;
    assert_eq!(checks(&named), expect(true));
    let (status, empty) = get_admin(&format!("/apps/{app_id}/functions?environment=dev-a1")).await;
    assert_eq!(
        (status, empty),
        (StatusCode::OK, json!([])),
        "serves no build"
    );
    let (status, body) = get_admin(&format!("/apps/{app_id}/functions?environment=nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_environment");
}

const SECRETS_APP: &str = "fn-env-check-secrets";
const ENV_JS: &str = "export default async (req, ctx) => Response.json({ env: ctx.env });";

/// A queued check reads the secrets of the environment it runs in. Staging's
/// `ctx.env` is staging's keys — its own value where production has the same
/// key, and never a key only production holds — so a check that passes there
/// did not pass on production's credentials. Production's run reads
/// production's, as it always has.
#[tokio::test]
async fn a_queued_check_reads_its_environments_secrets_not_productions() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );
    }
    let t = seeded_tenant().await;
    let reader = [FunctionSpec {
        name: "env",
        manifest: json!({ "check": true }),
        js: ENV_JS,
    }];
    let declared = json!({ "TOKEN": {}, "PRODUCTION_ONLY": {} });
    let app_id = publish_env_build(&t, SECRETS_APP, &reader, declared.clone(), "sec-1", true)
        .await
        .expect("production's build")
        .app_id;
    publish_env_build(&t, SECRETS_APP, &reader, declared, "sec-2", false)
        .await
        .expect("staging's build");
    let secrets = SecretManagerService::new(demo_workspace_id());
    for (environment, key, value) in [
        (None, "TOKEN", "production-token"),
        (None, "PRODUCTION_ONLY", "production-only"),
        (Some("staging"), "TOKEN", "staging-token"),
    ] {
        secrets
            .set_app_secret_in(&t.db, app_id, environment, key, value, t.guest_id)
            .await
            .expect("seed secret");
    }
    make_guest_staff();
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(t.db.clone(), platform);

    let mut env_of = Vec::new();
    for query in ["?environment=staging", ""] {
        let (status, queued) =
            post_admin(&format!("/apps/{app_id}/functions/env/runs{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query:?}: {queued}");
        let run_id = queued["run_id"].as_str().expect("a run id").to_string();
        let run = wait_for_run(app_id, &run_id).await;
        assert_eq!(run["status"], "done", "{query:?}: {run}");
        let answer: Value =
            serde_json::from_str(run["answer"].as_str().expect("an answer")).expect("JSON");
        env_of.push(answer["env"].clone());
    }
    driver.abort();

    assert_eq!(
        env_of[0],
        json!({ "TOKEN": "staging-token" }),
        "a staging check reads staging's keys and none of production's"
    );
    assert_eq!(
        env_of[1],
        json!({ "TOKEN": "production-token", "PRODUCTION_ONLY": "production-only" }),
        "a production run reads production's, as before"
    );
}
