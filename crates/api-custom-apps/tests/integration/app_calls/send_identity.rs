//! Who a delivered `ctx.apps.send` runs as (custom apps architecture §6.1):
//! the person who sent it, under the credential they sent it with and only
//! while that person may open the receiving app; otherwise nobody, or, for a
//! send no person made, the system.
//!
//! Rows here are written the way the host writes them, through the definer
//! function as the sender's writer and signed with the org's key
//! ([`forge_send`]), so each case isolates one rule: the relay's check of the
//! invoker MAC, the delivery's re-read of the credential, or its access check.
//! The first case takes the real road instead: a person's route call
//! publishes an event, and the listener that receives it sends.

use oxy_api_custom_apps::app_events::deliveries::SENDER_REFUSED;
use oxy_api_custom_apps::app_events::provenance::{self, Invoker, Provenance};
use oxy_api_custom_apps::app_events::relay::relay_once;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{SEEN_JS, answer, function, publish_reporter, writer_for};
use crate::app_events_delivery::{Fx, publish_app, with_fx};
use crate::custom_app_functions_fixture::invocations;

/// What a forged row's invoker MAC is.
#[derive(Clone, Copy)]
enum Mac {
    /// The host's, over the row and the invoker (message domain v2).
    Host,
    /// The row's own v1 MAC, standing in for one: what a server one release
    /// back signs, which never vouches for an invoker.
    V1,
}

/// A `ctx.apps.send` row from `sender`'s `go` to `callee`'s `handle` under
/// `key`, written as `sender`'s own writer through the definer, its v1 MAC
/// the host's, carrying `actor` and an invocation, and `invoker` with the MAC
/// `mac` says (none at all when `invoker` is `None`). Its id.
async fn forge_send(
    fx: &Fx,
    sender: &str,
    callee: &str,
    key: &str,
    actor: Uuid,
    invoker: Option<(Invoker, Mac)>,
) -> Uuid {
    use oxy_oltp::events::{self as outbox, OutboxTarget, PUBLISH_WITH_INVOKER_SQL};
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
    let name = oxy_custom_apps::calls::send_name(callee, "handle", "go");
    let type_ = oxy_custom_apps::events::qualified_type(sender, &name);
    let id = oxy_custom_apps::events::event_id(t.org_id, "production", &type_, key);
    let data = json!({ "n": 4 });
    let actor = actor.to_string();
    let invocation = Uuid::new_v4();
    let fields = Provenance {
        id,
        environment: "production",
        event_type: &type_,
        key,
        writer_role: &conn.role,
        actor: Some(&actor),
        root_id: id,
        causation_id: None,
        depth: 0,
        delay_seconds: 0,
        data_bytes: oxy_custom_apps::events::jsonb_text_len(&data) as i64,
        schema_revision: None,
        invocation_id: Some(invocation),
    };
    let row_mac = provenance::sign_hex(t.org_id, &fields);
    let (kind, token, invoker_mac) = match invoker {
        None => (String::new(), String::new(), String::new()),
        Some((invoker, mac)) => (
            invoker.kind().to_string(),
            invoker
                .token_id()
                .map(|t| t.to_string())
                .unwrap_or_default(),
            match mac {
                Mac::Host => provenance::sign_invoker_hex(t.org_id, &fields, invoker),
                Mac::V1 => row_mac.clone(),
            },
        ),
    };
    let (client, connection) = tokio_postgres::connect(&conn.dsn, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .query_one(
            PUBLISH_WITH_INVOKER_SQL,
            &[
                &id,
                &"production",
                &name,
                &key,
                &data,
                &None::<Uuid>,
                &Some(actor.as_str()),
                &None::<Uuid>,
                &id,
                &0i32,
                &Some(invocation),
                &0i32,
                &row_mac,
                &kind,
                &token,
                &invoker_mac,
            ],
        )
        .await
        .unwrap_or_else(|e| panic!("forge_send: {e:?}"));
    id
}

/// A personal token of `user` in the test's org: all-access, or bound to one
/// workspace grant (`member`) on a workspace that is not the test's. Its id.
async fn personal_token(fx: &Fx, user: Uuid, narrowed: bool) -> Uuid {
    let db = &fx.t.db;
    let id = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    entity::api_tokens::ActiveModel {
        id: Set(id),
        kind: Set("personal".into()),
        principal_user_id: Set(user),
        name: Set("send identity".into()),
        display_prefix: Set("oxy_pat_test".into()),
        last_four: Set("abcd".into()),
        token_hash: Set(Uuid::new_v4().as_bytes().to_vec()),
        all_access: Set(!narrowed),
        platform: Set(false),
        partner: Set(false),
        created_at: Set(now),
        created_by: Set(Some(user)),
        source: Set("ui".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("mint the token");
    if narrowed {
        let other = Uuid::new_v4();
        entity::workspaces::ActiveModel {
            id: Set(other),
            name: Set(format!("other-{other}")),
            org_id: Set(Some(fx.t.org_id)),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("another workspace in the org");
        entity::api_token_grants::ActiveModel {
            id: Set(Uuid::new_v4()),
            token_id: Set(id),
            kind: Set("workspace".into()),
            org_id: Set(fx.t.org_id),
            workspace_id: Set(Some(other)),
            role_ceiling: Set(Some("member".into())),
            app_id: Set(None),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("grant the token one other workspace");
    }
    id
}

/// A callee app with `handle` (reporting what it saw) and a sender app with
/// `go` that declares the call: their slugs and the callee's id.
async fn callee_and_sender(fx: &Fx) -> (String, String, Uuid) {
    let (callee, sender) = (format!("callee-{}", fx.tag), format!("sender-{}", fx.tag));
    let callee_id = publish_reporter(
        fx,
        &callee,
        &[function(
            "handle",
            json!({ "callable": true, "timeoutSeconds": 5, "events": { "publish": ["report"] } }),
            SEEN_JS,
        )],
    )
    .await;
    writer_for(fx, &sender).await;
    publish_app(
        &fx.t,
        &sender,
        fx.workspace,
        "1",
        &[function(
            "go",
            json!({}),
            "export default async () => new Response(\"{}\");",
        )],
        json!({ "calls": { callee.clone(): ["handle"] } }),
    )
    .await;
    (callee, sender, callee_id)
}

/// Relay the org's production outbox, then the one delivery of event `id`.
async fn relay_and_deliver(fx: &Fx, id: Uuid) -> entity::app_event_deliveries::Model {
    relay_once(&fx.t.db, fx.t.org_id, "production")
        .await
        .expect("relay");
    let delivery = entity::app_event_deliveries::Entity::find()
        .filter(entity::app_event_deliveries::Column::EventId.eq(id))
        .one(&fx.t.db)
        .await
        .unwrap()
        .expect("the send is delivered to its function");
    fx.run(&delivery).await;
    fx.reload(&delivery).await
}

/// The stored event's invoker columns.
async fn stored_invoker(fx: &Fx, id: Uuid) -> (Option<String>, Option<Uuid>) {
    let e = entity::app_events::Entity::find_by_id(id)
        .one(&fx.t.db)
        .await
        .unwrap()
        .expect("stored");
    (e.invoker, e.invoker_token_id)
}

/// The delivery was killed for its sender and the function never ran.
async fn assert_refused(fx: &Fx, delivery: &entity::app_event_deliveries::Model, callee_id: Uuid) {
    assert_eq!(delivery.status, "dead", "{delivery:?}");
    assert!(
        delivery
            .last_error
            .as_deref()
            .is_some_and(|e| e.starts_with(SENDER_REFUSED)),
        "{:?}",
        delivery.last_error
    );
    assert!(
        invocations(&fx.t.db, callee_id, "handle").await.is_empty(),
        "the function never ran"
    );
}

/// The review's first finding: a send made by a run that does not run as a
/// person (here an event's delivery) carries no invoker, though its chain's
/// actor is the person whose route call published the event, so it runs as
/// the system and presents no actor.
#[tokio::test]
async fn a_send_from_an_events_delivery_runs_as_the_system_whatever_its_actor() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, _, callee_id) = callee_and_sender(&fx).await;
        let (publisher, listener) = (format!("pinger-{}", fx.tag), format!("relayer-{}", fx.tag));
        writer_for(&fx, &publisher).await;
        publish_app(
            t,
            &publisher,
            fx.workspace,
            "1",
            &[function(
                "go",
                json!({ "events": { "publish": ["ping"] } }),
                r#"export default async (req, ctx) =>
  Response.json(await ctx.events.publish("ping", JSON.parse(req.body), { key: "p1" }));"#,
            )],
            json!({ "events": { "ping": { "schema": { "type": "object" } } } }),
        )
        .await;
        writer_for(&fx, &listener).await;
        publish_app(
            t,
            &listener,
            fx.workspace,
            "1",
            &[function(
                "on-ping",
                json!({ "on": [format!("{publisher}/ping")], "timeoutSeconds": 30 }),
                r#"export default async (req, ctx) => {
  const event = JSON.parse(req.body);
  await ctx.apps.send(event.data.target, "handle", { n: 3 }, { key: "from-event" });
  return Response.json({ ok: true });
};"#,
            )],
            json!({ "calls": { callee.clone(): ["handle"] } }),
        )
        .await;

        let ping = answer(&t.org_slug, &publisher, "go", json!({ "target": callee })).await;
        let ping: Uuid = ping["eventId"]
            .as_str()
            .expect("an event id")
            .parse()
            .unwrap();
        let stored = entity::app_events::Entity::find_by_id(ping)
            .one(&t.db)
            .await
            .unwrap();
        assert!(stored.is_none(), "not relayed yet");
        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay the ping");
        let ping_row = entity::app_events::Entity::find_by_id(ping)
            .one(&t.db)
            .await
            .unwrap()
            .expect("the ping");
        assert_eq!(
            ping_row.actor,
            Some(t.guest_id.to_string()),
            "the chain carries the person"
        );
        let to_listener = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(ping))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the listener's delivery");
        fx.run(&to_listener).await;
        assert_eq!(fx.reload(&to_listener).await.status, "acked");

        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay the send");
        let send = entity::app_events::Entity::find()
            .filter(entity::app_events::Column::CausationId.eq(ping))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the send, caused by the ping's delivery");
        assert_eq!(
            send.actor,
            Some(t.guest_id.to_string()),
            "it inherited the actor"
        );
        assert_eq!((send.invoker.clone(), send.invoker_token_id), (None, None));
        let delivery = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(send.id))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the send's delivery");
        fx.run(&delivery).await;

        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["kind"], "system", "{seen}");
        assert_eq!(seen["caller"]["actor"], Value::Null, "{seen}");
        let ran = invocations(&t.db, callee_id, "handle").await;
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert_ne!(ran[0].user_id, Some(t.guest_id));
    })
    .await;
}

/// A token the sender holds only on another workspace keeps that ceiling at
/// the delivery: the receiving app's workspace is outside it, so the send is
/// refused, though the person's own standing (a login session) opens the app.
/// The same send under an all-access token of theirs runs as them.
#[tokio::test]
async fn a_narrowed_credential_keeps_its_ceiling_at_the_delivery() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender, callee_id) = callee_and_sender(&fx).await;
        let narrowed = personal_token(&fx, t.guest_id, true).await;
        let open = personal_token(&fx, t.guest_id, false).await;

        let id = forge_send(
            &fx,
            &sender,
            &callee,
            "narrowed",
            t.guest_id,
            Some((Invoker::Token(narrowed), Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_eq!(
            stored_invoker(&fx, id).await,
            (Some("token".into()), Some(narrowed))
        );
        assert_refused(&fx, &delivery, callee_id).await;

        let id = forge_send(
            &fx,
            &sender,
            &callee,
            "open",
            t.guest_id,
            Some((Invoker::Token(open), Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_eq!(delivery.status, "acked", "{delivery:?}");
        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["kind"], "user", "{seen}");
        assert_eq!(
            seen["caller"]["actor"],
            json!(t.guest_id.to_string()),
            "{seen}"
        );
        let ran = invocations(&t.db, callee_id, "handle").await;
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert_eq!(ran[0].user_id, Some(t.guest_id));
    })
    .await;
}

/// A token revoked between the send and its delivery is refused: the delivery
/// is dead with the stable code, and the function never ran.
#[tokio::test]
async fn a_revoked_credential_is_refused_at_the_delivery() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender, callee_id) = callee_and_sender(&fx).await;
        let token = personal_token(&fx, t.guest_id, false).await;
        let id = forge_send(
            &fx,
            &sender,
            &callee,
            "revoked",
            t.guest_id,
            Some((Invoker::Token(token), Mac::Host)),
        )
        .await;
        let mut row: entity::api_tokens::ActiveModel =
            entity::api_tokens::Entity::find_by_id(token)
                .one(&t.db)
                .await
                .unwrap()
                .expect("the token")
                .into();
        row.revoked_at = Set(Some(chrono::Utc::now().fixed_offset()));
        row.update(&t.db).await.expect("revoke the token");

        let delivery = relay_and_deliver(&fx, id).await;
        assert_refused(&fx, &delivery, callee_id).await;
    })
    .await;
}

/// A person who may not open the receiving app is refused at the delivery,
/// whatever they could do when they sent it: here someone outside the org,
/// on a login session.
#[tokio::test]
async fn a_person_who_may_not_open_the_receiving_app_is_refused() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender, callee_id) = callee_and_sender(&fx).await;
        let outsider = Uuid::new_v4();
        entity::users::ActiveModel {
            id: Set(outsider),
            email: Set(Some(format!("outsider-{}@example.com", fx.tag))),
            name: Set("Outsider".into()),
            picture: Set(None),
            email_verified: Set(true),
            status: Set(entity::users::UserStatus::Active),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("a user in no org");
        let id = forge_send(
            &fx,
            &sender,
            &callee,
            "outsider",
            outsider,
            Some((Invoker::Session, Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_eq!(
            stored_invoker(&fx, id).await,
            (Some("session".into()), None)
        );
        assert_refused(&fx, &delivery, callee_id).await;
    })
    .await;
}

/// A row whose invoker the host did not sign under domain v2 — a server one
/// release back signs v1 alone, and app SQL can fill the columns through the
/// definer's settings — is the host's row all the same, and runs as the
/// system: its invoker is never stored, never presented.
#[tokio::test]
async fn a_v1_signed_send_runs_as_the_system_whatever_its_columns_say() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (callee, sender, callee_id) = callee_and_sender(&fx).await;
        let id = forge_send(
            &fx,
            &sender,
            &callee,
            "v1",
            t.guest_id,
            Some((Invoker::Session, Mac::V1)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_eq!(delivery.status, "acked", "{delivery:?}");
        assert_eq!(stored_invoker(&fx, id).await, (None, None));
        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["kind"], "system", "{seen}");
        assert_eq!(seen["caller"]["actor"], Value::Null, "{seen}");
        let ran = invocations(&t.db, callee_id, "handle").await;
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert_ne!(ran[0].user_id, Some(t.guest_id));
    })
    .await;
}

/// Staff reach into an org's app is platform standing on the live path (the
/// staff term of `user_can_access_app`, and `may_open_non_production` off
/// production), never an assume-role session, and a delivery re-reads it like
/// any other standing. A staff sender on a login session, in no org, runs as
/// themselves while the standing holds; once it ends, their next send is
/// refused at delivery: not run as the system, and not with the staff reach
/// they no longer hold. Two callees, so the second is decided afresh, not
/// from the first one's 60 s verdict.
#[tokio::test]
async fn a_staff_sender_is_refused_once_their_standing_has_ended() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (first, second, sender) = (
            format!("callee-{}", fx.tag),
            format!("callee2-{}", fx.tag),
            format!("sender-{}", fx.tag),
        );
        let handle = || {
            function(
                "handle",
                json!({ "callable": true, "timeoutSeconds": 5, "events": { "publish": ["report"] } }),
                SEEN_JS,
            )
        };
        let first_id = publish_reporter(&fx, &first, &[handle()]).await;
        let second_id = publish_reporter(&fx, &second, &[handle()]).await;
        writer_for(&fx, &sender).await;
        publish_app(
            t,
            &sender,
            fx.workspace,
            "1",
            &[function("go", json!({}), "export default async () => new Response(\"{}\");")],
            json!({ "calls": { first.clone(): ["handle"], second.clone(): ["handle"] } }),
        )
        .await;
        let staff = Uuid::new_v4();
        let email = format!("send-staff-{}@example.com", fx.tag);
        entity::users::ActiveModel {
            id: Set(staff),
            email: Set(Some(email.clone())),
            name: Set("Staff".into()),
            picture: Set(None),
            email_verified: Set(true),
            status: Set(entity::users::UserStatus::Active),
            ..Default::default()
        }
        .insert(&t.db)
        .await
        .expect("a staff user in no org");
        let now = chrono::Utc::now().fixed_offset();
        let grant = entity::app_admins::ActiveModel {
            id: Set(Uuid::new_v4()),
            email: Set(email),
            granted_by: Set(None),
            created_at: Set(now),
            role: Set("global_admin".into()),
            scope_all: Set(true),
            updated_at: Set(now),
        }
        .insert(&t.db)
        .await
        .expect("grant platform standing");
        oxy_server_authz::globals::invalidate_admin_cache();

        let id = forge_send(
            &fx,
            &sender,
            &first,
            "staff",
            staff,
            Some((Invoker::Session, Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_eq!(delivery.status, "acked", "{delivery:?}");
        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["kind"], "user", "{seen}");
        assert_eq!(seen["caller"]["actor"], json!(staff.to_string()), "{seen}");
        assert_eq!(
            invocations(&t.db, first_id, "handle").await[0].user_id,
            Some(staff)
        );

        entity::app_admins::Entity::delete_by_id(grant.id)
            .exec(&t.db)
            .await
            .expect("end the standing");
        oxy_server_authz::globals::invalidate_admin_cache();
        let id = forge_send(
            &fx,
            &sender,
            &second,
            "staff-ended",
            staff,
            Some((Invoker::Session, Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, id).await;
        assert_refused(&fx, &delivery, second_id).await;
    })
    .await;
}

/// A login session is checked at one hop only. Its end is not recorded, so
/// the 30-day bound measured from a send cannot follow a chain: a send made
/// by a run that was itself delivered as a session sender carries no invoker,
/// and the next function runs as the system. The first hop runs as the
/// person.
#[tokio::test]
async fn a_login_sessions_identity_stops_at_the_first_hop() {
    with_fx(|fx| async move {
        let t = &fx.t;
        let (dee, relay, sender) = (
            format!("dee-{}", fx.tag),
            format!("relay-{}", fx.tag),
            format!("sender-{}", fx.tag),
        );
        let dee_id = publish_reporter(
            &fx,
            &dee,
            &[function(
                "handle",
                json!({ "callable": true, "timeoutSeconds": 5, "events": { "publish": ["report"] } }),
                SEEN_JS,
            )],
        )
        .await;
        writer_for(&fx, &relay).await;
        let relay_js: &'static str = Box::leak(
            format!(
                r#"export default async (req, ctx) => {{
  await ctx.apps.send("{dee}", "handle", {{ n: 6 }}, {{ key: "hop2" }});
  return Response.json({{ ok: true }});
}};"#
            )
            .into_boxed_str(),
        );
        let relay_id = publish_app(
            t,
            &relay,
            fx.workspace,
            "1",
            &[function("handle", json!({ "callable": true, "timeoutSeconds": 5 }), relay_js)],
            json!({ "calls": { dee.clone(): ["handle"] } }),
        )
        .await;
        writer_for(&fx, &sender).await;
        publish_app(
            t,
            &sender,
            fx.workspace,
            "1",
            &[function("go", json!({}), "export default async () => new Response(\"{}\");")],
            json!({ "calls": { relay.clone(): ["handle"] } }),
        )
        .await;

        let first = forge_send(
            &fx,
            &sender,
            &relay,
            "hop1",
            t.guest_id,
            Some((Invoker::Session, Mac::Host)),
        )
        .await;
        let delivery = relay_and_deliver(&fx, first).await;
        assert_eq!(delivery.status, "acked", "{delivery:?}");
        let ran = invocations(&t.db, relay_id, "handle").await;
        assert_eq!(ran[0].user_id, Some(t.guest_id), "the first hop runs as the person");

        relay_once(&t.db, t.org_id, "production")
            .await
            .expect("relay the second send");
        let second = entity::app_events::Entity::find()
            .filter(entity::app_events::Column::CausationId.eq(first))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the relay's own send");
        assert_eq!(
            (second.invoker.clone(), second.invoker_token_id),
            (None, None),
            "a relayed session records no invoker"
        );
        let to_dee = entity::app_event_deliveries::Entity::find()
            .filter(entity::app_event_deliveries::Column::EventId.eq(second.id))
            .one(&t.db)
            .await
            .unwrap()
            .expect("the second send's delivery");
        fx.run(&to_dee).await;
        let seen = &fx.reported().await["seen"];
        assert_eq!(seen["kind"], "system", "{seen}");
        assert_eq!(seen["caller"]["actor"], Value::Null, "{seen}");
        let ran = invocations(&t.db, dee_id, "handle").await;
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert_ne!(ran[0].user_id, Some(t.guest_id));
    })
    .await;
}
