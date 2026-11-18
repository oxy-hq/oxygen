//! A sandbox on an OLTP branch of its own (planes `oltp` `branch`): its
//! events wait in that branch's outbox, and are relayed and delivered like
//! any other store's (R44's refusal lifted). The real host publishes through
//! the sandbox's own branch (cut for real by `LocalProvider`), the relay
//! claims from it, and the delivery reads the body there. The branch vouches
//! for its own sandbox only: a row in it claiming staging or production is
//! quarantined.

use agentic_core::delegation::TaskOutcome;
use axum::http::StatusCode;
use oxy_api_custom_apps::custom_apps_env_branch::task::{BranchOp, EnvBranchTask, run};
use oxy_oltp::WriterRef;
use oxy_oltp::resolver::{EnvBranchAccept, resolve_env_branch_writer_for_org};
use oxy_oltp::schema::app_writer_name;

use super::*;
use crate::custom_app_functions_fixture::call_function_with;

pub(super) const HANDLE: &str = "ob";
pub(super) const SANDBOX: &str = "dev-ob";

/// Publishes [`EVENT`] under the key its request names: inside a
/// `ctx.oltp.tx` when the request says `tx`.
const PUBLISH_JS: &str = r#"export default async (req, ctx) => {
  const { key, tx } = JSON.parse(req.body);
  const publish = (events) =>
    events.publish("statement.uploaded", { period: "2026-11" }, { key });
  return Response.json(
    tx ? await ctx.oltp.tx((handle) => publish(handle.events)) : await publish(ctx.events),
  );
};"#;

impl Fx {
    /// The publisher's sandbox [`SANDBOX`] on an OLTP branch of its own
    /// ([`Self::own_branch_sandbox_of`]).
    pub(super) async fn own_branch_sandbox(&self) {
        self.own_branch_sandbox_of(self.publisher, &self.slug).await;
    }

    /// App `app`'s (slug `slug`) sandbox [`SANDBOX`] on an OLTP branch of its
    /// own, cut, whose build publishes the app's [`EVENT`] from `pub` and
    /// listens to it with [`FUNCTION`], its events switch on.
    pub(super) async fn own_branch_sandbox_of(&self, app: Uuid, slug: &str) {
        let admin = crate::common::admin_url().await;
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var("OXY_OLTP_PROVIDER", "local");
            std::env::set_var("OXY_OLTP_ADMIN_URL", admin);
            std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com");
        }
        create_sandbox(&self.t, app, HANDLE, Some(json!({ "oltp": "branch" }))).await;
        let publisher = FunctionSpec {
            name: "pub",
            manifest: json!({
                "route": true,
                "timeoutSeconds": 60,
                "oltp": { "enabled": true },
                "events": { "publish": [EVENT] },
            }),
            js: PUBLISH_JS,
        };
        let listener = FunctionSpec {
            name: FUNCTION,
            manifest: json!({ "on": [qualified_type(slug, EVENT)], "timeoutSeconds": 30 }),
            js: OK_JS,
        };
        let build = format!("{slug}-{HANDLE}");
        let to = PublishInput {
            project_id: self.workspace,
            ..input(
                &self.t,
                slug,
                &build,
                bundle(slug, &[publisher, listener], catalog(), &[]),
            )
        };
        publish_to(to, PublishTarget::Sandbox(sandbox(HANDLE)))
            .await
            .unwrap_or_else(|e| panic!("publish {slug} to {SANDBOX}: {e}"));
        let cut = EnvBranchTask {
            app_id: app,
            app_slug: slug.to_string(),
            org_id: self.org(),
            workspace_id: self.workspace,
            environment: SANDBOX.to_string(),
            op: BranchOp::Cut { at: None },
        };
        run(&self.t.db, &cut)
            .await
            .unwrap_or_else(|e| panic!("cut {slug}'s branch: {e}"));
        self.switch_on(app, SANDBOX).await;
    }

    /// Another app, `<base>-<tag>`, publishing and listening to its own
    /// [`EVENT`] in its own [`SANDBOX`] on an OLTP branch of its own, its
    /// writer minted in the org's store. Its id and slug.
    pub(super) async fn second_own_branch_app(&self, base: &str) -> (Uuid, String) {
        let slug = format!("{base}-{}", self.tag);
        let writer = WriterRef::app(app_writer_name(&slug).expect("the slug backs a schema"))
            .expect("a valid writer name");
        self.store
            .ensure_extra_writer(&writer, self.workspace)
            .await;
        let app = promote(&self.t, &slug, self.workspace, &[], catalog()).await;
        self.own_branch_sandbox_of(app, &slug).await;
        (app, slug)
    }

    /// Call the publisher's `pub` in [`SANDBOX`] to publish [`EVENT`] under
    /// `key`; what it answered.
    pub(super) async fn publish_in_sandbox(&self, key: &str) -> Value {
        self.publish_in_sandbox_of(&self.slug, key).await
    }

    /// [`Self::publish_in_sandbox`] inside a `ctx.oltp.tx`.
    pub(super) async fn publish_in_sandbox_tx(&self, key: &str) -> Value {
        self.call_pub(&self.slug, json!({ "key": key, "tx": true }))
            .await
    }

    /// Call app `slug`'s `pub` in [`SANDBOX`] to publish its [`EVENT`] under
    /// `key`; what it answered.
    pub(super) async fn publish_in_sandbox_of(&self, slug: &str, key: &str) -> Value {
        self.call_pub(slug, json!({ "key": key })).await
    }

    /// Call app `slug`'s `pub` in [`SANDBOX`] with `body`; what it answered.
    async fn call_pub(&self, slug: &str, body: Value) -> Value {
        let headers = [("authorization", "Bearer t"), ("x-oxy-app-env", SANDBOX)];
        let call = call_function_with(&self.t.org_slug, slug, "pub", body, &headers).await;
        assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
        call.frame("data")
            .unwrap_or_else(|| panic!("no data frame: {}", call.raw))
            .clone()
    }

    /// The org's outbox mark for `environment`.
    async fn mark(&self, environment: &str) -> entity::app_event_outbox_marks::Model {
        entity::app_event_outbox_marks::Entity::find_by_id((self.org(), environment.to_string()))
            .one(&self.t.db)
            .await
            .expect("read the mark")
            .unwrap_or_else(|| panic!("{environment}'s outbox is marked"))
    }

    /// The publisher's writer on [`SANDBOX`]'s own branch.
    pub(super) async fn own_branch_writer(&self) -> oxy_oltp::resolver::WriterConnection {
        resolve_env_branch_writer_for_org(
            &self.t.db,
            self.org(),
            self.publisher,
            SANDBOX,
            &self.store.writer,
            EnvBranchAccept::Ready,
        )
        .await
        .expect("resolve the writer on the sandbox's branch")
        .expect("the sandbox has its branch")
        .connection
    }
}

/// The host publishes into the sandbox's own branch, the relay of
/// [`SANDBOX`] claims it from there, and the listener in that sandbox gets
/// it, its body read from the branch.
#[tokio::test]
async fn a_branch_sandbox_publish_is_relayed_and_delivered_from_its_own_branch() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        let answered = fx.publish_in_sandbox("statement:1").await;
        let id = event_id(fx.org(), SANDBOX, &fx.event_type(), "statement:1");
        assert_eq!(answered, json!({ "eventId": id.to_string() }));

        let stats = fx.relay(SANDBOX).await;
        assert_eq!((stats.relayed, stats.deliveries, stats.rejected), (1, 1, 0));
        let events = fx.events().await;
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(
            (
                events[0].id,
                events[0].environment.as_str(),
                &events[0].relay_error
            ),
            (id, SANDBOX, &None)
        );
        let target = OutboxTarget::EnvBranch {
            app_id: fx.publisher,
            environment: SANDBOX.to_string(),
        };
        let body = outbox::read_body(&fx.t.db, fx.org(), &target, &events[0].writer_role, id)
            .await
            .expect("read the body from the sandbox's branch");
        assert_eq!(body, Some(json!({ "period": "2026-11" })));

        let delivery = fx
            .delivery_to(fx.publisher)
            .await
            .expect("the sandbox listener's delivery");
        assert_eq!(
            (delivery.environment.as_str(), delivery.status.as_str()),
            (SANDBOX, "pending")
        );
        let outcome = fx.run(&delivery).await;
        let TaskOutcome::Done { answer, .. } = &outcome else {
            panic!("delivered: {outcome:?}");
        };
        assert_eq!(answer, "delivered (HTTP 200)");
        assert!(
            fx.relay("staging").await == RelayStats::default(),
            "nothing of the sandbox's is on the staging branch"
        );
    })
    .await;
}

/// A sandbox's own branch vouches for that sandbox only: a row in it
/// claiming staging or production — signed as the host would sign it, so
/// only the claim is wrong — is stored as the sandbox's, quarantined, and
/// delivered to nobody.
#[tokio::test]
async fn a_row_in_a_sandbox_branch_claiming_staging_or_production_is_quarantined() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        // The host's first publish there grants the writer on the branch.
        fx.publish_in_sandbox("statement:honest").await;
        let conn = fx.own_branch_writer().await;
        for claimed in ["staging", "production"] {
            let key = format!("claims:{claimed}");
            let id = event_id(fx.org(), claimed, &fx.event_type(), &key);
            fx.publish_signed(&conn, claimed, &key, id).await;
        }

        let stats = fx.relay(SANDBOX).await;
        assert_eq!((stats.relayed, stats.rejected), (1, 2), "{stats:?}");
        for claimed in ["staging", "production"] {
            let key = format!("claims:{claimed}");
            let stored = fx
                .events()
                .await
                .into_iter()
                .find(|e| e.key == key)
                .unwrap_or_else(|| panic!("the row claiming {claimed} is stored"));
            assert_eq!(stored.environment, SANDBOX, "the store decides");
            let error = stored.relay_error.as_deref().expect("a relay error");
            assert!(error.contains(&format!("`{claimed}`")), "{error}");
        }
        let deliveries = fx.deliveries().await;
        assert_eq!(
            deliveries.len(),
            1,
            "the honest event alone: {deliveries:?}"
        );
    })
    .await;
}

/// One store failing holds up no other (F1): a sandbox's pass reaches every
/// app's own branch of that name, in app-id order, and an app whose branch
/// cannot be opened (its owner's sealed password is unreadable: not a branch
/// that is gone) must not hold up the next app's events. That app's event is
/// relayed and delivered in the same pass; the pass still fails, naming the
/// broken store, and leaves the mark due with its wait going on, so the sweep
/// retries it and the stale alarm still sees it.
#[tokio::test]
async fn an_own_branch_that_cannot_be_opened_holds_up_no_other_apps_events() {
    with_fx(|fx| async move {
        fx.own_branch_sandbox().await;
        let (second, second_slug) = fx.second_own_branch_app("br-two").await;
        fx.publish_in_sandbox("statement:first").await;
        fx.publish_in_sandbox_of(&second_slug, "statement:second")
            .await;
        // Break the store the pass reaches first.
        let ((broken, _), (healthy, healthy_slug)) = {
            let mut apps = [(fx.publisher, fx.slug.clone()), (second, second_slug)];
            apps.sort();
            let [first, then] = apps;
            (first, then)
        };
        let healthy_key = if healthy == fx.publisher {
            "statement:first"
        } else {
            "statement:second"
        };
        fx.platform(
            "UPDATE oltp_env_branches SET owner_password_ciphertext = $1 \
              WHERE app_id = $2 AND env_name = $3",
            vec![
                b"not a sealed password".to_vec().into(),
                broken.into(),
                SANDBOX.into(),
            ],
        )
        .await;
        let pending = fx.mark(SANDBOX).await.pending_since;
        assert!(pending.is_some(), "the publishes left the mark due");

        let error = relay_once(&fx.t.db, fx.org(), SANDBOX)
            .await
            .expect_err("the broken store fails the pass");
        assert!(
            error.contains(&format!("app {broken}'s own `{SANDBOX}` branch")),
            "names the broken store: {error}"
        );
        let healthy_id = event_id(
            fx.org(),
            SANDBOX,
            &qualified_type(&healthy_slug, EVENT),
            healthy_key,
        );
        let relayed: Vec<_> = fx
            .events()
            .await
            .into_iter()
            .map(|e| (e.id, e.relay_error))
            .collect();
        assert_eq!(
            relayed,
            [(healthy_id, None)],
            "the healthy app's event alone"
        );
        let delivery = fx
            .delivery_to(healthy)
            .await
            .expect("the healthy app's listener's delivery");
        let outcome = fx.run(&delivery).await;
        let TaskOutcome::Done { answer, .. } = &outcome else {
            panic!("delivered: {outcome:?}");
        };
        assert_eq!(answer, "delivered (HTTP 200)");
        assert!(fx.delivery_to(broken).await.is_none());

        let mark = fx.mark(SANDBOX).await;
        assert!(
            mark.drained_at
                .is_none_or(|drained| mark.published_at > drained),
            "still due: {mark:?}"
        );
        assert_eq!(mark.pending_since, pending, "the wait goes on: {mark:?}");
    })
    .await;
}
