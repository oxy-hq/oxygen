//! The sandbox loop, start to cleanup, in one process
//! (`internal-docs/custom-app-sandboxes.md` §1.3): two sandboxes of one app
//! are created by route, each gets its own build, a function is called in
//! each over the serve route with `X-Oxy-App-Env`, a check runs in one
//! through the queue and the production executor, its invocation and held
//! list are read back by route, and one sandbox is deleted and torn down.
//!
//! The slices' own modules prove each piece alone. This one asserts them
//! **together**, on one app, in order:
//!
//! - each sandbox runs **its** build under **its** policy with **its**
//!   secrets and storage silo;
//! - an upload URL minted in one sandbox is sent from that sandbox and held
//!   from the other and from staging;
//! - nothing of one is visible from the other, from staging or from
//!   production — builds, secrets, objects, invocation rows, held writes;
//! - production's and staging's pointers never move;
//! - after the teardown the name is free and inherits nothing, and the other
//!   sandbox is intact.
//!
//! The app's own database is the loop's second test (`oltp`): on an app with
//! an OLTP store, each sandbox reads and writes a schema of its own.
//!
//! `scripts/ci/platform-canary-sandbox-loop.mjs` drives the same loop through
//! `oxyc` against a running server; what it cannot see — one sandbox's
//! object from the other — is asserted here.
//!
//! The upload step drives each environment's host from Rust
//! (`nonprod_function_uploads::fixture`), not through the route: a function's
//! `ctx.fetch` sends only HTTPS to a public host, so no in-process object
//! store can answer it, and what is asserted is that the PUT **leaves the
//! host** for the URL minted in the sandbox's silo — not that bytes landed.
//! The byte round trip is the canary's `storage_roundtrip`, on a deployment
//! whose object store is reached over HTTPS.
//!
//! **Needs** Postgres only.

mod fixture;
mod oltp;

use agentic_core::delegation::TaskOutcome;
use axum::http::StatusCode;
use entity::apps;
use oxy_app::server::api::custom_apps_publish::{PublishTarget, publish_to};
use serde_json::{Value, json};

use self::fixture::{
    A, APP, B, app_on_two_builds, call_in, fixed_pointers, functions, probe, ran_in, set_secret,
    set_token, silo_key, tarball,
};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::custom_app_functions_manual_run::{
    get_admin, platform, post_admin, spawn_driver, wait_for_run,
};
use crate::nonprod_function_uploads::fixture::{Rig, assert_held, assert_sent, mint, put};
use crate::sandbox_publish::{input, sandbox};
use crate::sandbox_routes::send;
use crate::sandbox_teardown_task::{objects, row, run_queued, secret, use_scratch_homes};
use crate::staging_functions::held_rows;

/// Steps 1–2 of the loop: both sandboxes created by route with no build,
/// then a different build published to each, with its own secrets.
async fn create_and_publish(t: &Tenant, app: &apps::Model) {
    let environments = format!("{}/environments", app.id);
    for name in [A, B] {
        let (status, body) = send("POST", &environments, Some(json!({ "name": name }))).await;
        assert_eq!(status, StatusCode::CREATED, "{name}: {body}");
        assert_eq!(
            (&body["name"], &body["build_id"]),
            (&json!(name), &Value::Null)
        );
    }
    for (handle, build, mark) in [
        ("a1", "loop-a", functions("a")),
        ("b2", "loop-b", functions("b")),
    ] {
        let target = PublishTarget::Sandbox(sandbox(handle));
        let published = publish_to(input(t, APP, build, tarball(&mark)), target)
            .await
            .expect("publish to the sandbox");
        assert_eq!(published.channel, "sandbox");
        assert_eq!(published.environment, format!("dev-{handle}"));
        let (_, shown) = send(
            "GET",
            &format!("{}/environments/dev-{handle}", app.id),
            None,
        )
        .await;
        assert_eq!(shown["build_id"], build, "{shown}");
    }
    set_token(t, app, Some(A), "a1-token").await;
    set_secret(t, app, Some(A), "ONLY_A", "yes").await;
    set_token(t, app, Some(B), "b2-token").await;
}

/// Step 3: a route call in every environment. Each answers from its own
/// build, as its own channel, with its own secrets, and lists only what it
/// stored itself.
async fn each_environment_runs_its_own(t: &Tenant, app: &apps::Model) {
    let a = probe(t, A, Some("notes/a.txt")).await;
    let b = probe(t, B, Some("notes/b.txt")).await;
    let expect = |build: &str, channel: &str, token: &str, only_a: Value, list: Value| json!({ "build": build, "channel": channel, "token": token, "only_a": only_a, "list": list });
    let (a_key, b_key) = (
        silo_key(app.id, A, "notes/a.txt"),
        silo_key(app.id, B, "notes/b.txt"),
    );
    let without_put = |mut answer: Value| {
        answer.as_object_mut().expect("an object").remove("put");
        answer
    };
    assert_eq!((&a["put"], &b["put"]), (&json!(a_key), &json!(b_key)));
    assert_eq!(
        without_put(a),
        expect("a", A, "a1-token", json!("yes"), json!([a_key]))
    );
    assert_eq!(
        without_put(b),
        expect("b", B, "b2-token", Value::Null, json!([b_key]))
    );
    // Staging and production run their own builds and see neither sandbox's
    // secret or object.
    assert_eq!(
        probe(t, "staging", None).await,
        expect("staging", "staging", "stg-token", Value::Null, json!([]))
    );
    assert_eq!(
        probe(t, "production", None).await,
        expect(
            "production",
            "production",
            "prod-token",
            Value::Null,
            json!([])
        )
    );
}

/// Steps 4–5: a check queued in `dev-a1` by route and run by the production
/// executor, then read back by route. Returns the check's invocation id.
async fn a_check_runs_in_its_sandbox(t: &Tenant, app: &apps::Model) -> String {
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(t.db.clone(), platform);
    let runs = format!("/apps/{}/functions/smoke/runs?environment={A}", app.id);
    let (status, queued) = post_admin(&runs).await;
    assert_eq!(status, StatusCode::OK, "queue the check: {queued}");
    let run = wait_for_run(app.id, queued["run_id"].as_str().expect("a run id")).await;
    driver.abort();

    assert_eq!(
        (&run["status"], &run["environment"]),
        (&json!("done"), &json!(A)),
        "{run}"
    );
    let answer: Value =
        serde_json::from_str(run["answer"].as_str().expect("an answer")).expect("JSON");
    assert_eq!(
        answer,
        json!({
            "build": "a", "channel": A, "token": "a1-token", "write": 409,
            "list": [silo_key(app.id, A, "notes/a.txt")],
        }),
        "the check ran dev-a1's build, under its policy, with its secret and silo"
    );
    let invocation = run["invocation_id"]
        .as_str()
        .expect("an invocation")
        .to_string();
    let (status, held) =
        get_admin(&format!("/apps/{}/invocations/{invocation}/held", app.id)).await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(
        (&held["environment"], &held["function"], &held["build_id"]),
        (&json!(A), &json!("smoke"), &json!("loop-a")),
        "{held}"
    );
    let ops: Vec<&str> = held["held"]
        .as_array()
        .expect("held")
        .iter()
        .filter_map(|w| w["op"].as_str())
        .collect();
    assert_eq!(ops, vec!["fetch"], "{held}");
    invocation
}

/// What each environment's read-back shows after the calls and the check:
/// its own rows on its own build, and the one held write, filed under
/// `dev-a1` alone.
async fn read_backs_are_each_environments_own(t: &Tenant, app: &apps::Model) {
    let on = |function: &str, build: &str| (function.to_string(), build.to_string());
    assert_eq!(
        ran_in(app.id, A).await,
        vec![on("probe", "loop-a"), on("smoke", "loop-a")]
    );
    assert_eq!(ran_in(app.id, B).await, vec![on("probe", "loop-b")]);
    assert_eq!(
        ran_in(app.id, "staging").await,
        vec![on("probe", "loop-stg")]
    );
    assert_eq!(
        ran_in(app.id, "production").await,
        vec![on("probe", "loop-prod")]
    );
    let held: Vec<String> = held_rows(t)
        .await
        .into_iter()
        .map(|r| r.environment)
        .collect();
    assert_eq!(held, vec![A.to_string()], "one held write, dev-a1's");
}

/// Between the read-backs and the delete: an upload is its sandbox's own.
/// `dev-a1` mints an upload URL into its silo and PUTs to it — sent, and not
/// a held write; `dev-b2` and staging each send an upload of their own and,
/// handed `dev-a1`'s URL, hold it, each in a held row of its own. `dev-a1`'s
/// silo is left as it was.
async fn an_upload_is_its_sandboxes_own(t: &Tenant, app: &apps::Model) {
    use oxy_app_core::custom_app_environment::AppEnvironment;

    let rig = Rig::of(t, app.id).await;
    let a = rig.invocation(sandbox("a1"));
    let upload = mint(&*a, "uploads/a.bin").await;
    assert!(
        upload.key.starts_with(&silo_key(app.id, A, "uploads/")),
        "{}",
        upload.key
    );
    assert_sent(&put(&*a, &upload.url).await, "dev-a1's own upload");
    let listed = a
        .storage("list".into(), json!({}))
        .await
        .expect("list dev-a1's silo");
    let keys: Vec<&str> = listed["objects"]
        .as_array()
        .expect("objects")
        .iter()
        .filter_map(|o| o["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        vec![silo_key(app.id, A, "notes/a.txt")],
        "nothing landed, and the silo is as it was"
    );
    a.end_of_invocation().await;

    for (environment, other) in [(B, sandbox("b2")), ("staging", AppEnvironment::Staging)] {
        let host = rig.invocation(other);
        let own = mint(&*host, "uploads/own.bin").await;
        assert_sent(&put(&*host, &own.url).await, environment);
        assert_held(&put(&*host, &upload.url).await, environment);
        host.end_of_invocation().await;
    }
    let mut held: Vec<String> = held_rows(t)
        .await
        .into_iter()
        .map(|r| r.environment)
        .collect();
    held.sort();
    assert_eq!(
        held,
        vec![A.to_string(), B.to_string(), "staging".to_string()],
        "dev-a1's row is still the check's one: its upload added none"
    );
}

/// Step 6: `dev-a1` deleted by route and its teardown run. It stops serving
/// at once; its silo, secrets and row go; its history stays; the name is
/// free and the sandbox created under it inherits nothing; `dev-b2` is
/// exactly as it was.
async fn delete_and_tear_down(t: &Tenant, app: &apps::Model) {
    let (status, deleting) = send("DELETE", &format!("{}/environments/{A}", app.id), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{deleting}");
    assert_eq!(deleting["status"], "deleting");
    let gone = call_in(t, A, "probe", json!({})).await;
    assert_eq!(
        gone.status,
        StatusCode::NOT_FOUND,
        "a deleting sandbox serves nothing: {}",
        gone.raw
    );

    let run_id = deleting["teardown_run_id"]
        .as_str()
        .expect("a teardown run");
    let outcome = run_queued(&t.db, run_id).await;
    assert!(
        matches!(outcome, TaskOutcome::Done { .. }),
        "the teardown failed: {outcome:?}"
    );
    assert_eq!(row(&t.db, app.id, A).await, None, "the row is gone");
    assert_eq!(objects(app.id, &sandbox("a1")).await, 0);
    assert_eq!(secret(app, A).await, None);
    let on = |function: &str| (function.to_string(), "loop-a".to_string());
    assert_eq!(
        ran_in(app.id, A).await,
        vec![on("probe"), on("smoke")],
        "history is kept"
    );

    let (status, again) = send(
        "POST",
        &format!("{}/environments", app.id),
        Some(json!({ "name": A })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "the name is free: {again}");
    assert_eq!(again["build_id"], Value::Null, "and starts with no build");
    assert_eq!(
        call_in(t, A, "probe", json!({})).await.status,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        row(&t.db, app.id, B).await,
        Some(false),
        "dev-b2 is still active"
    );
    assert_eq!(objects(app.id, &sandbox("b2")).await, 1);
    assert_eq!(secret(app, B).await.as_deref(), Some("b2-token"));
    let b = probe(t, B, None).await;
    assert_eq!(
        (&b["build"], &b["token"], &b["list"]),
        (
            &json!("b"),
            &json!("b2-token"),
            &json!([silo_key(app.id, B, "notes/b.txt")])
        )
    );
}

#[tokio::test]
async fn two_sandboxes_run_the_whole_loop_apart_from_each_other_staging_and_production() {
    let tmp = use_scratch_homes();
    let t = seeded_tenant().await;
    let app = app_on_two_builds(&t).await;
    let fixed = fixed_pointers(&t, app.id).await;

    create_and_publish(&t, &app).await;
    assert_eq!(
        fixed_pointers(&t, app.id).await,
        fixed,
        "two sandbox publishes moved nothing fixed"
    );
    each_environment_runs_its_own(&t, &app).await;
    a_check_runs_in_its_sandbox(&t, &app).await;
    read_backs_are_each_environments_own(&t, &app).await;
    an_upload_is_its_sandboxes_own(&t, &app).await;
    delete_and_tear_down(&t, &app).await;

    assert_eq!(
        fixed_pointers(&t, app.id).await,
        fixed,
        "production and staging never moved"
    );
    assert_eq!(probe(&t, "staging", None).await["build"], "staging");
    assert_eq!(probe(&t, "production", None).await["build"], "production");
    let _ = std::fs::remove_dir_all(tmp);
}
