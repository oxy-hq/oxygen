//! `ctx.apps.send` on the host: the same call as `ctx.apps.call`, queued
//! instead of awaited. It is written as a directed Phase 1 event in the org's
//! outbox, through the writer the app publishes as (`events_op`), so it needs
//! the org's OLTP store as any publish does, and it is relayed, delivered,
//! retried and dead-lettered as an event is — to the one function it names.
//!
//! The row's name is [`send_name`] (`call:<app>:<function>:<caller function>`),
//! which the relay reads as a directed event: it resolves the callee in the
//! environment the row was published in, and stores the event with a relay error
//! (delivered to nobody, and visible in the events API) when the app, function
//! or build is missing or the function is not callable. There is no schema; the
//! body is JSON under the event payload limit. The callee receives the body as
//! its request, with `ctx.caller` set, as a call would give it.
//!
//! The answer is `{ eventId }`, the id of the event to look up. The key is the
//! caller's `key` when given (a repeat of one is the same send), else a new one
//! per send; `delaySeconds` is 0 to 86 400 as for any event.

use oxy_custom_apps::calls::send_name;
use oxy_custom_apps::events::{MAX_KEY_LEN, MAX_PAYLOAD_BYTES, jsonb_text_len};
use serde_json::{Value, json};

use super::apps_op::Target;
use super::events_op::{Checked, delay_seconds, holds_nul};
use super::*;

const KEY_INVALID: &str = "AppCallRefused: `key` must be 1–200 characters without U+0000";

impl ProjectFunctionHost {
    /// `ctx.apps.send`: the checked call, queued. See the module docs.
    pub(super) async fn send(
        &self,
        host_op: HostOp,
        target: Target,
        payload: &Value,
    ) -> Result<Value, String> {
        let checked = self.check_send(&target, payload).await?;
        let decided = self.policy.decide(host_op);
        let outbox = self.admit_publish(host_op, decided, &checked.name).await?;
        let conn = self.publish_writer(host_op, &outbox).await?;
        let (id, params) = self.publish_params(&checked, &conn.role);
        self.publish_alone(&outbox, &conn, &params).await?;
        self.after_publish_commit().await;
        Ok(json!({ "eventId": id.to_string() }))
    }

    /// The checks a send adds to a call's: the callee app exists, the key and
    /// delay are in bounds and the body fits an event.
    async fn check_send(&self, target: &Target, payload: &Value) -> Result<Checked, String> {
        let app = self.find_target_app(&target.app).await?;
        // A person is asked now; at delivery there is no one to ask.
        self.authorize_target(&app).await?;
        let key = match payload.get("key") {
            None | Some(Value::Null) => uuid::Uuid::new_v4().to_string(),
            Some(Value::String(key))
                if (1..=MAX_KEY_LEN).contains(&key.chars().count()) && !key.contains('\0') =>
            {
                key.clone()
            }
            _ => return Err(KEY_INVALID.to_string()),
        };
        let delay_seconds = delay_seconds(payload.get("delaySeconds"))
            .map_err(|e| e.replacen("EventDelayInvalid", "AppCallRefused", 1))?;
        let bytes = jsonb_text_len(&target.body);
        if bytes > MAX_PAYLOAD_BYTES || holds_nul(&target.body) {
            return Err(format!(
                "AppCallRefused: a send's body is held to an event's: {MAX_PAYLOAD_BYTES} bytes \
                 of JSON without U+0000 (this one is {bytes} bytes)"
            ));
        }
        Ok(Checked {
            name: send_name(&target.app, &target.function, &self.identity.function_name),
            key,
            data: target.body.clone(),
            delay_seconds,
            data_bytes: bytes,
            revision: None,
        })
    }
}
