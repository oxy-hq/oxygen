//! What the host op hands the isolate, over a real Postgres: the gate the
//! host asks first (`platform_facts::admit`, against the deployment's
//! internal orgs), then the reply, cached, and the one message a failure
//! carries.

use std::sync::Arc;

use entity::organizations;
use oxy_api_custom_apps::custom_apps_functions::InvocationIdentity;
use oxy_api_custom_apps::custom_apps_functions::env_policy::HostOp;
use oxy_api_custom_apps::custom_apps_functions::host::platform_facts::{
    UNAVAILABLE, admit, build_platform_facts, cached_reply, load_reply,
};
use oxy_api_custom_apps::custom_apps_functions::host::{
    FunctionCapabilities, ProjectFunctionHost, into_arc,
};
use oxy_api_custom_apps::custom_apps_functions::runtime::FunctionHost;
use oxy_api_custom_apps::custom_apps_functions::seam::{
    FunctionProjectContext, FunctionQueryExecutor,
};
use oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor;
use oxy_app_core::internal_orgs::INTERNAL_ORGS_ENV;
use oxy_app_core::operating_graph::reach::Reach;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection};
use uuid::Uuid;

use super::world::{World, now};
use crate::common::{Schema, fresh_db};
use crate::staging_homes_fixture::{duck, staging, workspace};

fn org_json(reply: &serde_json::Value, id: Uuid) -> &serde_json::Value {
    reply["orgs"]
        .as_array()
        .expect("orgs is a list")
        .iter()
        .find(|o| o["id"] == id.to_string())
        .unwrap_or_else(|| panic!("org {id} missing from the reply"))
}

/// The internal app lives in an internal org: it passes the gate and gets
/// every org, in the contract's camelCase. A customer's app that declares the
/// capability is still refused.
#[tokio::test]
async fn an_internal_org_gets_the_facts_and_a_customer_org_does_not() {
    let w = World::new().await;
    // nextest runs each test in a process of its own, so this env is ours.
    unsafe { std::env::set_var(INTERNAL_ORGS_ENV, w.staff_org.to_string()) };

    let refused = admit(true, w.acme).unwrap_err();
    assert!(
        refused.starts_with("PlatformFactsNotEnabled: "),
        "{refused}"
    );
    let internal = admit(true, w.staff_org).expect("the staff org passes");
    assert_eq!(
        internal,
        [w.staff_org],
        "the gate hands on the list it read"
    );

    let reply = load_reply(&w.db, internal, 2, now()).await.expect("facts");
    assert_eq!(
        reply["weeks"],
        serde_json::json!(["2026-09-28", "2026-10-05"])
    );
    assert_eq!(reply["generatedAt"], "2026-10-14T05:00:00Z");
    assert_eq!(org_json(&reply, w.staff_org)["internal"], true);
    let acme = org_json(&reply, w.acme);
    assert_eq!(acme["internal"], false);
    assert_eq!(acme["weekly"].as_array().map(Vec::len), Some(2));
    assert_eq!(acme["last7d"]["appViews"], 3);
}

/// Within the cache's window a second call is answered without a build: an
/// org created between the two calls is not in the second reply.
#[tokio::test]
async fn a_second_call_inside_the_window_is_answered_from_the_cache() {
    let w = World::new().await;
    let internal = [w.staff_org];
    let first = cached_reply(&w.db, &internal, 2).await.expect("facts");
    let late = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(late),
        name: Set("Late Co".into()),
        slug: Set(format!("facts-{}", late.simple())),
        ..Default::default()
    }
    .insert(&w.db)
    .await
    .expect("seed org");

    let second = cached_reply(&w.db, &internal, 2).await.expect("facts");
    assert_eq!(second, first, "the same reply, not a rebuild");
    let fresh = load_reply(&w.db, &internal, 2, now()).await.expect("facts");
    assert!(
        fresh["orgs"].as_array().unwrap().len() > first["orgs"].as_array().unwrap().len(),
        "a build would have seen the new org"
    );
}

/// A database the build cannot read (the central schema alone lacks the
/// agentic runtime's run columns) fails it with Postgres's own text, naming a
/// column, and the function sees none of it.
#[tokio::test]
async fn a_database_error_reaches_the_function_as_the_fixed_message() {
    let db = fresh_db(Schema::Central).await.0;
    let internal = [Uuid::new_v4()];
    let raw = build_platform_facts(&db, &internal, 2, now())
        .await
        .expect_err("the central schema alone cannot answer the agent-run read")
        .to_string();
    assert!(
        raw.contains("does not exist"),
        "the build's own error carries Postgres's text: {raw}"
    );
    let reply = load_reply(&db, &internal, 2, now()).await;
    assert_eq!(reply, Err(UNAVAILABLE.to_string()));
}

/// Through the real host, in **staging**: both gates are asked before the
/// environment policy, so an org that is not internal is refused as not enabled,
/// never as held, and the staff org reads the facts there as in production.
#[tokio::test]
async fn in_staging_the_host_refuses_by_its_gates_and_serves_the_staff_org() {
    let w = World::new().await;
    // nextest runs each test in a process of its own, so this env is ours.
    unsafe { std::env::set_var(INTERNAL_ORGS_ENV, w.staff_org.to_string()) };
    let ws = workspace(&duck("w")).await;
    let call = |org: Uuid, granted: bool| {
        let host = staging_host(&w.db, ws.ctx(), org, granted);
        async move { host.platform_facts(serde_json::json!({ "weeks": 2 })).await }
    };

    let refused = call(w.acme, true).await.unwrap_err();
    assert!(
        refused.starts_with("PlatformFactsNotEnabled: "),
        "{refused}"
    );
    // The capability gate is the broker's, asked before the op is
    // dispatched (`runtime::host_call`); a direct call on the host bypasses
    // it, so it is asked the way the broker asks it.
    let undeclared = staging_host(&w.db, ws.ctx(), w.staff_org, false)
        .capabilities()
        .refuse(HostOp::PlatformFacts)
        .unwrap_err();
    assert!(
        undeclared.starts_with("PlatformCapabilityMissing: "),
        "{undeclared}"
    );
    let facts = call(w.staff_org, true)
        .await
        .expect("the staff org reads the facts in staging");
    assert_eq!(facts["weeks"].as_array().map(Vec::len), Some(2));
}

/// A staging host for an app in `org`, granted `platform.facts` or not.
fn staging_host(
    db: &DatabaseConnection,
    project: Arc<dyn FunctionProjectContext>,
    org: Uuid,
    granted: bool,
) -> Arc<dyn FunctionHost> {
    let caps = FunctionCapabilities {
        platform_facts: granted,
        ..Default::default()
    };
    into_arc(ProjectFunctionHost::new(
        project,
        Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
        db.clone(),
        Vec::new(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        org,
        Uuid::nil(),
        "Accounts".into(),
        caps,
        Default::default(),
        Reach::nowhere(),
        InvocationIdentity {
            invocation_id: Uuid::new_v4(),
            function_name: "facts".into(),
            mode: "route".into(),
            request_id: None,
            app_slug: "accounts".into(),
            user_id: None,
            user_email: None,
            credential_token_id: None,
        },
        staging(&[]),
    ))
}
