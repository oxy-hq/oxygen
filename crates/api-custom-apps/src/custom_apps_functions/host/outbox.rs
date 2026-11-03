//! `ctx.email.send` and `ctx.notify` under the messages plane's `capture`
//! home (`Isolate(Target::Outbox)`): the message is rendered as it would have
//! gone out, sent to nobody, and kept in the environment's outbox
//! (`custom_apps_outbox`).
//!
//! Capture needs no invoking user, so a scheduled run captures where the
//! invoker home would refuse it. It still counts against the silo's storage
//! limit; a refusal fails the call the way a refused `ctx.storage.put` does,
//! and a message is never sent instead.

use chrono::Utc;
use oxy_app::surface::emails::app_emailer::{AppEmailer, EmailSendInput, RenderedEmail};
use oxy_app_core::notifications::input::NotifyInput;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::super::data_audit::InvocationIdentity;
use super::super::env_policy::HostOp;
use super::ProjectFunctionHost;
use crate::custom_apps_outbox::{self as outbox, Message};

impl ProjectFunctionHost {
    /// Keep a `ctx.email.send` in the outbox. The payload is validated and
    /// composed by the emailer, so one production refuses is refused here.
    pub(super) async fn capture_email(
        &self,
        emailer: &AppEmailer,
        parsed: EmailSendInput,
    ) -> Result<Value, String> {
        let rendered = emailer.render(parsed)?;
        let key = self
            .keep(HostOp::EmailSend, email_message(rendered, &self.identity))
            .await?;
        Ok(json!({
            "messageId": format!("captured-{}", Uuid::new_v4()),
            "captured": true,
            "environment": self.policy.environment().name(),
            "deliveredTo": [],
            "outboxKey": key,
        }))
    }

    /// Keep a `ctx.notify` in the outbox. Nobody is resolved and no store is
    /// written: an org with no OLTP store can still try its notifications.
    pub(super) async fn capture_notify(&self, parsed: &NotifyInput) -> Result<Value, String> {
        let key = self
            .keep(HostOp::Notify, notify_message(parsed, &self.identity))
            .await?;
        Ok(json!({
            "captured": true,
            "environment": self.policy.environment().name(),
            "recipients": 0,
            "skipped": [],
            "deduplicated": false,
            "outboxKey": key,
        }))
    }

    /// Write `message` into this environment's outbox, behind the silo's
    /// storage limit. Returns the object's key.
    async fn keep(&self, op: HostOp, message: Message) -> Result<String, String> {
        let Some(silo) = self.policy.outbox_silo(self.app_id) else {
            return Err(self.misrouted(op));
        };
        let (pathname, bytes) = message.encode(Utc::now(), Uuid::new_v4())?;
        self.storage_limit(&silo, bytes.len() as u64).await?;
        outbox::write(&silo, &pathname, bytes, &self.caps.storage_retention)
            .await
            .map_err(|e| e.to_string())
    }
}

/// A message with the fields both kinds share; the caller adds the rest.
fn message(kind: &'static str, to: Value, identity: &InvocationIdentity) -> Message {
    Message {
        kind,
        to,
        subject: None,
        title: None,
        body: None,
        invocation_id: identity.invocation_id,
        function: identity.function_name.clone(),
        captured_at: String::new(),
        details: Map::new(),
    }
}

/// An email as the outbox keeps it: everyone the call named (nobody was
/// reached), the body in the form the local preview prefers, the attachments
/// by name.
fn email_message(rendered: RenderedEmail, identity: &InvocationIdentity) -> Message {
    let mut out = message("email", json!(rendered.to), identity);
    out.subject = Some(rendered.subject);
    let (body, format) = match (rendered.html, rendered.text) {
        (Some(html), _) => (Some(html), "html"),
        (None, text) => (text, "text"),
    };
    out.body = body;
    out.details.insert("format".into(), json!(format));
    let optional = [
        ("cc", json!(rendered.cc)),
        ("bcc", json!(rendered.bcc)),
        ("attachments", json!(rendered.attachments)),
    ];
    for (field, list) in optional {
        if list.as_array().is_some_and(|l| !l.is_empty()) {
            out.details.insert(field.into(), list);
        }
    }
    if let Some(reply_to) = rendered.reply_to {
        out.details.insert("reply_to".into(), json!(reply_to));
    }
    out
}

/// A notification as the outbox keeps it: the targets as the call wrote them.
fn notify_message(parsed: &NotifyInput, identity: &InvocationIdentity) -> Message {
    let to = parsed
        .targets
        .iter()
        .map(|(_, wrote)| wrote.clone())
        .collect();
    let mut out = message("notify", Value::Array(to), identity);
    out.title = Some(parsed.title.clone());
    out.body = parsed.body.clone();
    out.details.insert("notify_kind".into(), json!(parsed.kind));
    if let Some(link) = &parsed.link {
        out.details.insert("link".into(), json!(link));
    }
    out
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
