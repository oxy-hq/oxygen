//! `ctx.notify` — tell people in this org something.
//!
//! The function names people (a user, a role at a location, a team) and the
//! platform does the rest: who that is ([`recipients`]), where the content is
//! kept and the inbox row written ([`send`]), and push, email and Slack per
//! person on the queue. The function never handles an address.
//!
//! Gates, in order: the `notify` capability (the broker's, from the op's
//! registry row; [`NOTIFY_CAPABILITY_MISSING`] is its words), the
//! per-invocation cap, the payload, the kind (declared in the manifest — it
//! is what a person mutes), the org's content store, then the environment. Outside production a
//! notification reaches the invoking user alone, title prefixed `[<env>]`, the way `ctx.email.send`
//! does; a run with no invoking user (a schedule) is refused there rather than
//! told to somebody nobody asked.
//!
//! [`recipients`]: oxy_app_core::notifications::recipients
//! [`send`]: oxy_app_core::notifications::send

use serde_json::{Value, json};

use super::super::env_policy::{self, HostOp, Target};
use super::ProjectFunctionHost;
use oxy_app_core::notifications::compose;
use oxy_app_core::notifications::input::{self, EmailRequest, NotifyInput};
use oxy_app_core::notifications::recipients::{self, Skipped};
use oxy_app_core::notifications::send::{
    AppRef, EmailContent, Outgoing, SendError, send, thread_key,
};

/// `ctx.notify` calls one invocation may make — the same bound as email: a
/// loop over rows belongs in one call naming many people, not many calls.
pub(super) const MAX_NOTIFY_PER_INVOCATION: usize = 20;

/// What a function without the `notify` capability sees
/// (`FunctionCapabilities::refuse`).
pub(super) const NOTIFY_CAPABILITY_MISSING: &str = "NotifyCapabilityMissing: this function has not declared the `notify` capability \
     (add \"notify\": { \"kinds\": { \"<kind>\": {} } } to its oxy-app.json entry)";

const NO_INVOKER_FIX: &str = "a notification outside production goes only to the invoking \
     user, and this run has none (a schedule, an Airway step or a manual run); call the \
     function from the app to try it";

impl ProjectFunctionHost {
    pub(super) async fn notify_op(&self, payload: Value) -> Result<Value, String> {
        // The gate is the broker's; the kinds are what it granted.
        let Some(kinds) = &self.caps.notify_kinds else {
            return Err(super::gate_bypassed(HostOp::Notify));
        };
        let n = self
            .notify_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if n > MAX_NOTIFY_PER_INVOCATION {
            return Err(format!(
                "TooManyNotifications: this invocation exceeded the \
                 {MAX_NOTIFY_PER_INVOCATION}-notification limit — name everyone in one call's `to`"
            ));
        }
        let parsed = input::parse(&payload)?;
        let Some(&critical) = kinds.get(&parsed.kind) else {
            return Err(format!(
                "NotifyKindUndeclared: kind `{kind}` is not declared for this function (add \
                 \"notify\": {{ \"kinds\": {{ \"{kind}\": {{}} }} }} to its oxy-app.json entry) \
                 — a kind is what a person mutes, so each one is named up front",
                kind = parsed.kind
            ));
        };
        // Held where the policy holds it (a production run reading a branch):
        // answered, not thrown, the way a held email is.
        if self.holds(HostOp::Notify) {
            self.note_held(HostOp::Notify, ("notify", "", "SEND", &parsed.kind))
                .await;
            return Ok(env_policy::held_notify_result(
                &payload,
                &self.held_error(HostOp::Notify),
            ));
        }
        // Captured, not sent: nobody is resolved and no store is written, so an
        // org with none can still try its notifications.
        let home = self.isolated_home(HostOp::Notify);
        if home == Some(Target::Outbox) {
            return self.capture_notify(&parsed).await;
        }
        // Before anyone is resolved: an org with no store to keep the content
        // in cannot notify anyone, and saying so costs one control-plane read.
        oxy_oltp::inbox::ensure_writable(&self.db, self.org_id)
            .await
            .map_err(|e| match e.is_not_provisioned() {
                true => format!("NotifyStoreMissing: {e}"),
                false => format!("NotifyFailed: {e}"),
            })?;
        self.notify_in_home(home, parsed, critical).await
    }

    /// Send where the policy's home for `ctx.notify` says: as asked in
    /// production, to the invoking user elsewhere.
    async fn notify_in_home(
        &self,
        home: Option<Target>,
        parsed: NotifyInput,
        critical: bool,
    ) -> Result<Value, String> {
        let target = ("notify", "", "SEND", "");
        match home {
            None => self.notify_as_asked(parsed, critical).await,
            Some(Target::InvokerNotify) if !self.policy.is_production() => {
                let Some(invoker) = self.identity.user_id else {
                    self.note_held(HostOp::Notify, target).await;
                    return Err(env_policy::refused_message(
                        HostOp::Notify,
                        self.policy.environment(),
                        NO_INVOKER_FIX,
                    ));
                };
                self.notify_invoker(parsed, critical, invoker).await
            }
            Some(_) => Err(self.misrouted(HostOp::Notify)),
        }
    }

    /// Production: everyone the call names, within the app's audience.
    async fn notify_as_asked(&self, parsed: NotifyInput, critical: bool) -> Result<Value, String> {
        let (users, skipped) =
            recipients::resolve(&self.db, self.org_id, self.app_id, &parsed.targets).await?;
        let sent = self.send_to(parsed, critical, users, "").await?;
        Ok(sent_json(&sent, &skipped, None))
    }

    /// Outside production: the invoking user alone, whoever the call named.
    async fn notify_invoker(
        &self,
        parsed: NotifyInput,
        critical: bool,
        invoker: uuid::Uuid,
    ) -> Result<Value, String> {
        let environment = self.policy.environment().name();
        // Resolved anyway, so the answer says who production would have told.
        let (would_reach, _) =
            recipients::resolve(&self.db, self.org_id, self.app_id, &parsed.targets).await?;
        let prefix = format!("[{environment}] ");
        let sent = self
            .send_to(parsed, critical, vec![invoker], &prefix)
            .await?;
        tracing::info!(
            %environment,
            would_reach = would_reach.len(),
            "ctx.notify delivered to the invoking user only"
        );
        Ok(sent_json(
            &sent,
            &[],
            Some(json!({
                "environment": environment,
                "deliveredTo": [invoker.to_string()],
                "wouldReach": would_reach.len(),
            })),
        ))
    }

    async fn send_to(
        &self,
        parsed: NotifyInput,
        critical: bool,
        users: Vec<uuid::Uuid>,
        prefix: &str,
    ) -> Result<oxy_app_core::notifications::send::Sent, String> {
        let title = format!("{prefix}{}", parsed.title);
        let email = self.compose_email(&parsed, &title, prefix).await;
        let out = Outgoing {
            org_id: self.org_id,
            app: Some(AppRef {
                id: self.app_id,
                slug: self.identity.app_slug.clone(),
                name: self.app_name.clone(),
            }),
            environment: self.policy.environment().name(),
            workspace_id: Some(self.project_id),
            kind: parsed.kind,
            critical,
            title,
            body: parsed.body,
            link: parsed.link,
            email,
            slack: parsed.slack,
            idempotency_key: parsed.idempotency_key,
            thread_key: parsed
                .thread
                .as_deref()
                .map(|t| thread_key(Some(self.app_id), t)),
            recipients: users,
        };
        send(&self.db, out).await.map_err(|e| match e {
            SendError::NoStore(e) => format!("NotifyStoreMissing: {e}"),
            other => format!("NotifyFailed: {other}"),
        })
    }

    /// The email a call asked for ([`compose::email`]), with this app as the
    /// sender and its environment's host in the link.
    async fn compose_email(
        &self,
        parsed: &NotifyInput,
        title: &str,
        prefix: &str,
    ) -> Option<EmailContent> {
        if matches!(parsed.email, EmailRequest::None) {
            return None;
        }
        let org_slug = self.org_slug().await;
        let sender = compose::Sender {
            app_name: &self.app_name,
            app_slug: &self.identity.app_slug,
            org_slug: org_slug.as_deref(),
            environment: self.policy.environment(),
        };
        compose::email(&sender, parsed, title, prefix)
    }

    /// The app's org slug, for a link that leaves the product. `None` when it
    /// cannot be read: the email then goes without a button rather than not
    /// at all.
    async fn org_slug(&self) -> Option<String> {
        use sea_orm::EntityTrait;
        entity::organizations::Entity::find_by_id(self.org_id)
            .one(&self.db)
            .await
            .ok()
            .flatten()
            .map(|o| o.slug)
    }
}

fn sent_json(
    sent: &oxy_app_core::notifications::send::Sent,
    skipped: &[Skipped],
    environment: Option<Value>,
) -> Value {
    let mut out = json!({
        "notificationId": sent.id.to_string(),
        "recipients": sent.recipients,
        "skipped": skipped
            .iter()
            .map(|s| json!({ "target": s.target, "reason": s.reason }))
            .collect::<Vec<_>>(),
        "deduplicated": sent.deduplicated,
    });
    if let (Some(obj), Some(Value::Object(env))) = (out.as_object_mut(), environment) {
        obj.extend(env);
    }
    out
}
