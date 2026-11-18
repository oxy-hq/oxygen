//! App-to-app calls (`ctx.apps.call`, `ctx.apps.send`; app events design §8),
//! through the real road: apps published into an org, the function route, the
//! host op, the callee's admission, its isolate, and the invocation rows.
//!
//! A call: the callee runs as itself under the system identity with
//! `ctx.caller` naming the caller, answers its JSON, and its invocation row has
//! mode `call` and the caller's invocation as its parent. Every refusal is a
//! typed `AppCallError`. A chain of calls stops at the depth limit, and under a
//! pool smaller than the chain it ends in `busy` and releases its permits
//! instead of stalling. A send is a directed event: it is relayed to the one
//! callable function it names, which runs as a call with the sent body.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use oxy_api_custom_apps::app_events::relay::relay_once;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::app_events_delivery::{publish_app, with_fx};
use crate::custom_app_functions_fixture::{FunctionSpec, call_function_in, invocations};

/// Answers with what it was handed and what it is told about its caller.
const CALLEE_JS: &str = r#"
export default async (req, ctx) => Response.json({
  body: JSON.parse(req.body), caller: ctx.caller, kind: ctx.user.kind, email: ctx.user.email,
});
"#;

/// Calls the pair its body names; answers the callee's answer or the typed
/// error it threw.
const CALLER_JS: &str = r#"
export default async (req, ctx) => {
  const { app, fn, body, opts } = JSON.parse(req.body);
  try {
    return Response.json({ ok: await ctx.apps.call(app, fn, body ?? null, opts) });
  } catch (e) {
    return Response.json({ err: { name: e.name, kind: e.kind, message: e.message } });
  }
};
"#;

/// Calls itself one level deeper until told to stop; reports where a call
/// failed and why.
const HOP_JS: &str = r#"
export default async (req, ctx) => {
  const { n } = JSON.parse(req.body);
  try {
    const inner = await ctx.apps.call("loop", "hop", { n: n + 1 });
    return Response.json({ n, inner });
  } catch (e) {
    return Response.json({ n, failed: e.kind, message: e.message });
  }
};
"#;

fn callable() -> Value {
    json!({ "callable": true, "timeoutSeconds": 5 })
}

fn function(name: &'static str, manifest: Value, js: &'static str) -> FunctionSpec {
    FunctionSpec { name, manifest, js }
}

/// What a route call to `app`/`go` answered: its `data` frame.
async fn answer(org: &str, app: &str, function: &str, body: Value) -> Value {
    let call = call_function_in(org, app, function, body).await;
    call.frame("data")
        .cloned()
        .unwrap_or_else(|| panic!("no data frame: {}", call.raw))
}

#[tokio::test]
async fn a_call_runs_the_callee_as_itself_and_records_the_parent() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let callee = format!("callee-{}", fx.tag);
        let poster = format!("poster-{}", fx.tag);
        let callee_id = publish_app(
            t,
            &callee,
            fx.workspace,
            "1",
            &[function("get-mappings", callable(), CALLEE_JS)],
            json!({}),
        )
        .await;
        let poster_id = publish_app(
            t,
            &poster,
            fx.workspace,
            "1",
            &[function("go", json!({}), CALLER_JS)],
            json!({ "calls": { callee.clone(): ["get-mappings"] } }),
        )
        .await;

        let got = answer(
            &t.org_slug,
            &poster,
            "go",
            json!({ "app": callee, "fn": "get-mappings", "body": { "n": 1 } }),
        )
        .await;

        let rows = invocations(&t.db, poster_id, "go").await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        let ok = &got["ok"];
        assert_eq!(ok["body"], json!({ "n": 1 }), "{got}");
        assert_eq!(
            ok["kind"], "user",
            "a call from a person runs as that person"
        );
        assert_eq!(ok["email"], json!(oxy_auth::user::LOCAL_GUEST_EMAIL));
        assert_eq!(ok["caller"]["app"], json!(poster));
        assert_eq!(ok["caller"]["function"], "go");
        assert_eq!(ok["caller"]["invocationId"], json!(rows[0].id.to_string()));

        let called = invocations(&t.db, callee_id, "get-mappings").await;
        assert_eq!(called.len(), 1, "{called:?}");
        assert_eq!(called[0].mode, "call");
        assert_eq!(called[0].parent_invocation_id, Some(rows[0].id));
        assert_eq!(called[0].user_id, Some(t.guest_id));
        assert_eq!(called[0].status, "success");
        assert_eq!(
            called[0].result_body, None,
            "a call's answer is kept nowhere"
        );
        assert_eq!(rows[0].parent_invocation_id, None);
    })
    .await;
}

#[tokio::test]
async fn every_refusal_is_a_typed_error() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, closed, poster) = (
            format!("callee-{}", fx.tag),
            format!("closed-{}", fx.tag),
            format!("poster-{}", fx.tag),
        );
        publish_app(
            t,
            &callee,
            fx.workspace,
            "1",
            &[
                function("open", callable(), CALLEE_JS),
                function("private", json!({}), CALLEE_JS),
                function(
                    "throws",
                    callable(),
                    "export default async () => { throw new Error('nope '.repeat(300)); };",
                ),
            ],
            json!({}),
        )
        .await;
        publish_app(
            t,
            &poster,
            fx.workspace,
            "1",
            &[function("go", json!({}), CALLER_JS)],
            json!({ "calls": {
                callee.clone(): ["open", "private", "throws", "missing"],
                "ghost": ["x"],
                closed.clone(): ["x"],
            } }),
        )
        .await;
        let kind = |app: String, f: &'static str| {
            let (org, poster) = (t.org_slug.clone(), poster.clone());
            async move {
                answer(&org, &poster, "go", json!({ "app": app, "fn": f })).await["err"].clone()
            }
        };
        let undeclared = kind(callee.clone(), "unlisted").await;
        assert_eq!(
            (undeclared["name"].as_str(), undeclared["kind"].as_str()),
            (Some("AppCallError"), Some("refused"))
        );
        assert!(
            undeclared["message"]
                .as_str()
                .unwrap()
                .contains("does not declare a call")
        );
        assert_eq!(
            kind(callee.clone(), "private").await["kind"],
            "not_callable"
        );
        assert_eq!(kind(callee.clone(), "missing").await["kind"], "not_found");
        assert_eq!(kind("ghost".into(), "x").await["kind"], "not_found");
        let thrown = kind(callee.clone(), "throws").await;
        assert_eq!(thrown["kind"], "failed");
        assert!(
            thrown["message"].as_str().unwrap().chars().count() <= 520,
            "{thrown}"
        );
        // A person cannot open an app with no production build, so the call is
        // refused at the target's door before deployment is asked.
        crate::custom_app_functions_fixture::publish_build(
            t,
            &closed,
            fx.workspace,
            "staging-only",
            false,
            &[function("x", callable(), CALLEE_JS)],
        )
        .await;
        assert_eq!(kind(closed.clone(), "x").await["kind"], "refused");
        assert_eq!(
            answer(
                &t.org_slug,
                &poster,
                "go",
                json!({ "app": callee, "fn": "open", "opts": { "timeoutMs": 0 } })
            )
            .await["err"]["kind"],
            "refused"
        );
    })
    .await;
}

#[tokio::test]
async fn a_chain_stops_at_the_depth_limit() {
    // Room for the whole chain, so the depth limit is what stops it.
    unsafe {
        std::env::set_var("OXY_FUNCTION_MAX_CHAIN_IN_FLIGHT", "16");
    }
    with_fx(|fx| async move {
        publish_app(
            &fx.t,
            "loop",
            fx.workspace,
            "1",
            &[function("hop", callable(), HOP_JS)],
            json!({ "calls": { "loop": ["hop"] } }),
        )
        .await;
        let mut level = answer(&fx.t.org_slug, "loop", "hop", json!({ "n": 0 })).await;
        let mut depth = 0;
        while level.get("inner").is_some() {
            level = level["inner"].clone();
            depth += 1;
        }
        assert_eq!(level["failed"], "depth_exceeded", "{level}");
        assert_eq!(
            depth, 9,
            "the route is outside the chain; its callees sit at depths 0 to 8, the limit"
        );
    })
    .await;
}

/// Two nested isolates under a chain of nine: a callee that finds no nested
/// permit is refused at once as busy (it never waits for capacity only its own
/// kind can free), the chain unwinds, and the permits come back.
#[tokio::test]
async fn a_chain_deeper_than_the_pool_ends_busy_and_releases_its_permits() {
    unsafe {
        std::env::set_var("OXY_FUNCTION_MAX_CONCURRENCY", "3");
        std::env::set_var("OXY_FUNCTION_MAX_ORG_CONCURRENCY", "3");
        std::env::set_var("OXY_FUNCTION_MAX_NESTED_CONCURRENCY", "2");
        std::env::set_var("OXY_FUNCTION_MAX_ORG_NESTED_CONCURRENCY", "2");
        std::env::set_var("OXY_FUNCTION_MAX_CHAIN_IN_FLIGHT", "16");
    }
    with_fx(|fx| async move {
        publish_app(
            &fx.t,
            "loop",
            fx.workspace,
            "1",
            &[
                function("hop", callable(), HOP_JS),
                function("leaf", callable(), CALLEE_JS),
                function("probe", json!({}), CALLER_JS),
            ],
            json!({ "calls": { "loop": ["hop", "leaf"] } }),
        )
        .await;
        let started = Instant::now();
        let mut level = answer(&fx.t.org_slug, "loop", "hop", json!({ "n": 0 })).await;
        while level.get("inner").is_some() {
            level = level["inner"].clone();
        }
        assert_eq!(level["failed"], "busy", "{level}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "it did not stall"
        );

        let after = answer(
            &fx.t.org_slug,
            "loop",
            "probe",
            json!({ "app": "loop", "fn": "leaf", "body": 1 }),
        )
        .await;
        assert_eq!(after["ok"]["body"], 1, "the permits were released: {after}");
    })
    .await;
}

/// Reports what it was handed by publishing it as `report` (a send's callee
/// answers nobody), keyed `seen`.
const SEEN_JS: &str = r#"
export default async (req, ctx) => {
  await ctx.events.publish("report", { body: JSON.parse(req.body), caller: ctx.caller, kind: ctx.user.kind, attempt: ctx.delivery.attempt }, { key: "seen" });
  return Response.json({ ok: true });
};
"#;

const SENDER_JS: &str = r#"
export default async (req, ctx) => {
  const { app, fn } = JSON.parse(req.body);
  try {
    return Response.json(await ctx.apps.send(app, fn, { n: 2 }, { key: "s1", delaySeconds: 0 }));
  } catch (e) {
    return Response.json({ err: { kind: e.kind, message: e.message } });
  }
};
"#;

/// An app that can report what it saw: its own writer in the org's store and
/// a `report` event it may publish.
async fn publish_reporter(
    fx: &crate::app_events_delivery::Fx,
    slug: &str,
    functions: &[FunctionSpec],
) -> uuid::Uuid {
    let writer = oxy_oltp::WriterRef::app(
        oxy_oltp::schema::app_writer_name(slug).expect("the slug backs a schema"),
    )
    .expect("a writer");
    fx.store.ensure_extra_writer(&writer, fx.workspace).await;
    publish_app(
        &fx.t,
        slug,
        fx.workspace,
        "1",
        functions,
        json!({ "events": { "report": { "schema": { "type": "object" } } } }),
    )
    .await
}

#[tokio::test]
async fn a_send_is_relayed_to_the_one_function_and_runs_as_a_call() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender) = (format!("callee-{}", fx.tag), format!("sender-{}", fx.tag));
        let callee_id = publish_reporter(
            &fx,
            &callee,
            &[function(
                "handle",
                json!({ "callable": true, "timeoutSeconds": 5, "events": { "publish": ["report"] } }),
                SEEN_JS,
            )],
        )
        .await;
        let sender_writer = oxy_oltp::WriterRef::app(
            oxy_oltp::schema::app_writer_name(&sender).expect("a writer name"),
        )
        .expect("a writer");
        fx.store.ensure_extra_writer(&sender_writer, fx.workspace).await;
        let sender_id = publish_app(
            t,
            &sender,
            fx.workspace,
            "1",
            &[function("go", json!({}), SENDER_JS)],
            json!({ "calls": { callee.clone(): ["handle"] } }),
        )
        .await;

        let sent = answer(&t.org_slug, &sender, "go", json!({ "app": callee, "fn": "handle" })).await;
        let event_id: uuid::Uuid = sent["eventId"].as_str().expect("an event id").parse().unwrap();
        relay_once(&t.db, t.org_id, "production").await.expect("relay");

        let delivery = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(event_id))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the send is delivered to its function");
        assert_eq!((delivery.app_id, delivery.function_name.as_str()), (callee_id, "handle"));
        let outcome = fx.run(&delivery).await;
        assert!(matches!(outcome, agentic_core::delegation::TaskOutcome::Done { .. }), "{outcome:?}");

        let caller_run = invocations(&t.db, sender_id, "go").await;
        let ran = invocations(&t.db, callee_id, "handle").await;
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert_eq!(ran[0].mode, "call");
        assert_eq!(ran[0].parent_invocation_id, Some(caller_run[0].id));
        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["body"], json!({ "n": 2 }), "the callee gets the sent body, not an envelope");
        assert_eq!(seen["kind"], "system");
        assert_eq!(seen["caller"]["app"], json!(sender));
        assert_eq!(seen["caller"]["function"], "go");
        assert_eq!(seen["caller"]["invocationId"], json!(caller_run[0].id.to_string()));
    })
    .await;
}

#[tokio::test]
async fn a_send_to_a_function_that_is_not_callable_is_stored_and_delivered_to_nobody() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender) = (format!("callee-{}", fx.tag), format!("sender-{}", fx.tag));
        publish_app(
            t,
            &callee,
            fx.workspace,
            "1",
            &[function("handle", json!({}), CALLEE_JS)],
            json!({}),
        )
        .await;
        let sender_writer = oxy_oltp::WriterRef::app(
            oxy_oltp::schema::app_writer_name(&sender).expect("a writer name"),
        )
        .expect("a writer");
        fx.store
            .ensure_extra_writer(&sender_writer, fx.workspace)
            .await;
        publish_app(
            t,
            &sender,
            fx.workspace,
            "1",
            &[function("go", json!({}), SENDER_JS)],
            json!({ "calls": { callee.clone(): ["handle"] } }),
        )
        .await;
        let sent = answer(
            &t.org_slug,
            &sender,
            "go",
            json!({ "app": callee, "fn": "handle" }),
        )
        .await;
        let event_id: uuid::Uuid = sent["eventId"]
            .as_str()
            .expect("an event id")
            .parse()
            .unwrap();
        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay");

        let stored = entity::app_events::Entity::find_by_id(event_id)
            .one(&t.db)
            .await
            .unwrap()
            .expect("the event is stored");
        assert!(
            stored
                .relay_error
                .as_deref()
                .is_some_and(|e| e.contains("callable")),
            "{:?}",
            stored.relay_error
        );
        let deliveries = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(event_id))
            .all(&t.db)
            .await
            .unwrap();
        assert!(deliveries.is_empty(), "{deliveries:?}");
    })
    .await;
}

/// An outbox row `name` written as `sender`'s own writer through the definer
/// function, with the actor `user:evil`, and signed as the host signs a row
/// (R25(c)): so the relay refuses it, when it does, for the call it names and
/// never for its provenance — what a host bug letting a send through would
/// write.
async fn forge(
    fx: &crate::app_events_delivery::Fx,
    sender: &str,
    name: &str,
    key: &str,
) -> uuid::Uuid {
    use oxy_oltp::events::{self as outbox, OutboxTarget, PUBLISH_SQL};
    let t = &fx.t;
    let writer =
        oxy_oltp::WriterRef::app(oxy_oltp::schema::app_writer_name(sender).expect("a writer name"))
            .expect("a writer");
    let conn = oxy_oltp::resolver::resolve_writer_connection_for_org(&t.db, t.org_id, &writer)
        .await
        .expect("resolve the sender's writer");
    outbox::ensure_publishable(&t.db, t.org_id, &OutboxTarget::Production, &conn.role)
        .await
        .expect("grant the definer function");
    let type_ = oxy_custom_apps::events::qualified_type(sender, name);
    let id = oxy_custom_apps::events::event_id(t.org_id, "production", &type_, key);
    let data = json!({ "n": 1 });
    let mac = oxy_api_custom_apps::app_events::provenance::sign_hex(
        t.org_id,
        &oxy_api_custom_apps::app_events::provenance::Provenance {
            id,
            environment: "production",
            event_type: &type_,
            key,
            writer_role: &conn.role,
            actor: Some("user:evil"),
            root_id: id,
            causation_id: None,
            depth: 0,
            delay_seconds: 0,
            data_bytes: oxy_custom_apps::events::jsonb_text_len(&data) as i64,
            schema_revision: None,
            invocation_id: None,
        },
    );
    let (client, connection) = tokio_postgres::connect(&conn.dsn, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .query_one(
            PUBLISH_SQL,
            &[
                &id,
                &"production",
                &name,
                &key,
                &data,
                &None::<uuid::Uuid>,
                &Some("user:evil"),
                &None::<uuid::Uuid>,
                &id,
                &0i32,
                &None::<uuid::Uuid>,
                &0i32,
                &mac,
            ],
        )
        .await
        .unwrap_or_else(|e| panic!("forge: {e:?}"));
    id
}

async fn writer_for(fx: &crate::app_events_delivery::Fx, slug: &str) {
    let w = oxy_oltp::WriterRef::app(oxy_oltp::schema::app_writer_name(slug).expect("a name"))
        .expect("a writer");
    fx.store.ensure_extra_writer(&w, fx.workspace).await;
}

/// The relay enforces what the host enforces: a row naming a callable target
/// is delivered only when the sender's own build declares the call from the
/// function the row names. A row from an app that declares nothing, or from a
/// function it does not have, is stored with a relay error and delivered to
/// nobody, whoever the row says it is.
#[tokio::test]
async fn a_forged_send_row_is_refused_by_the_relay_and_a_declared_one_is_delivered() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let callee = format!("callee-{}", fx.tag);
        let (honest, rogue) = (format!("honest-{}", fx.tag), format!("rogue-{}", fx.tag));
        publish_app(
            t,
            &callee,
            fx.workspace,
            "1",
            &[function("handle", callable(), CALLEE_JS)],
            json!({}),
        )
        .await;
        for slug in [&honest, &rogue] {
            writer_for(&fx, slug).await;
        }
        publish_app(
            t,
            &honest,
            fx.workspace,
            "1",
            &[function("go", json!({}), SENDER_JS)],
            json!({ "calls": { callee.clone(): ["handle"] } }),
        )
        .await;
        publish_app(
            t,
            &rogue,
            fx.workspace,
            "1",
            &[function("go", json!({}), SENDER_JS)],
            json!({}),
        )
        .await;

        let name = |from: &str| oxy_custom_apps::calls::send_name(&callee, "handle", from);
        let forged = forge(&fx, &rogue, &name("go"), "f1").await;
        let wrong_fn = forge(&fx, &honest, &name("ghost"), "f2").await;
        let declared = forge(&fx, &honest, &name("go"), "f3").await;
        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay");

        let state = |id: uuid::Uuid| {
            let db = t.db.clone();
            async move {
                let e = entity::app_events::Entity::find_by_id(id)
                    .one(&db)
                    .await
                    .unwrap()
                    .expect("stored");
                let d = entity::app_event_deliveries::Entity::find()
                    .filter(entity::app_event_deliveries::Column::EventId.eq(id))
                    .all(&db)
                    .await
                    .unwrap();
                (e.relay_error, d.len())
            }
        };
        let (error, deliveries) = state(forged).await;
        assert!(
            error
                .as_deref()
                .is_some_and(|e| e.contains("does not declare")),
            "{error:?}"
        );
        assert_eq!(deliveries, 0);
        let (error, deliveries) = state(wrong_fn).await;
        assert!(
            error.is_some(),
            "a caller function the sender does not have is refused"
        );
        assert_eq!(deliveries, 0);
        let (error, deliveries) = state(declared).await;
        assert_eq!(
            (error, deliveries),
            (None, 1),
            "a declared send is delivered"
        );
    })
    .await;
}

/// A send-named event queued under any trigger but `call` is killed, not run
/// as a listener; its delivery is dead and the function never ran.
#[tokio::test]
async fn a_send_named_task_with_the_event_trigger_is_killed() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender) = (format!("callee-{}", fx.tag), format!("sender-{}", fx.tag));
        let callee_id = publish_app(
            t,
            &callee,
            fx.workspace,
            "1",
            &[function("handle", callable(), CALLEE_JS)],
            json!({}),
        )
        .await;
        writer_for(&fx, &sender).await;
        publish_app(
            t,
            &sender,
            fx.workspace,
            "1",
            &[function("go", json!({}), SENDER_JS)],
            json!({ "calls": { callee.clone(): ["handle"] } }),
        )
        .await;
        let id = forge(
            &fx,
            &sender,
            &oxy_custom_apps::calls::send_name(&callee, "handle", "go"),
            "k",
        )
        .await;
        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay");
        let delivery = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(id))
            .one(&t.db)
            .await
            .unwrap()
            .expect("delivered");
        let (mut spec, _) = fx.task(&delivery).await;
        if let agentic_core::delegation::TaskSpec::Custom { payload, .. } = &mut spec {
            payload["trigger"] = json!("event");
        }
        let outcome = crate::env_schedules::run_spec(&t.db, spec).await;
        assert!(
            matches!(outcome, agentic_core::delegation::TaskOutcome::Done { .. }),
            "{outcome:?}"
        );
        let after = entity::app_event_deliveries::Entity::find_by_id(delivery.id)
            .one(&t.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, "dead");
        assert!(
            after
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("is not a send to")),
            "killed for the trigger mismatch, not refused later: {:?}",
            after.last_error
        );
        assert!(
            invocations(&t.db, callee_id, "handle").await.is_empty(),
            "the function never ran"
        );
    })
    .await;
}
