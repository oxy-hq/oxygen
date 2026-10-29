//! A notification action that runs an app function, as the person who
//! tapped it.
//!
//! Not a second way to run a function: the call goes through the app's own
//! function route (the runtime in `oxy-api-custom-apps`, reached through
//! `FunctionRuntime`) exactly
//! as `POST /customer-apps/<org>/<app>/fn/<name>` would, on the caller's own
//! credential, so authentication, app access, the environment gate, the
//! function's capability gates, its rate limit, the invocation row and its
//! logs all apply. The route answers an SSE stream; this reads its terminal
//! frame back into JSON.
//!
//! An idempotency key per (notification, action) makes a double tap run the
//! function once: the second answers with the first's result.

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde_json::json;
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy_app_core::function_answer::{Call, call};
pub use oxy_app_core::function_answer::{Outcome, read_stream};
use oxy_app_core::function_seam::FunctionRuntime;

/// One function action, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCall {
    pub org_slug: String,
    pub app_slug: String,
    pub function: String,
    pub notification_id: Uuid,
    pub action: String,
    /// The notification's app-relative link, as the app gave it.
    pub link: Option<String>,
}

impl FunctionCall {
    /// What the function receives as its request body.
    pub fn body(&self) -> Bytes {
        Bytes::from(
            json!({
                "notificationId": self.notification_id,
                "action": self.action,
                "link": self.link,
            })
            .to_string(),
        )
    }

    pub fn idempotency_key(&self) -> String {
        format!(
            "notification-action:{}:{}",
            self.notification_id, self.action
        )
    }

    fn as_call(&self) -> Call<'_> {
        Call {
            org_slug: &self.org_slug,
            app_slug: &self.app_slug,
            function: &self.function,
            body: self.body(),
            idempotency_key: Some(self.idempotency_key()),
        }
    }

    pub fn uri(&self) -> String {
        self.as_call().uri()
    }

    /// The caller's headers for the call (`function_answer::Call::headers`).
    pub fn headers(&self, caller: &HeaderMap) -> HeaderMap {
        self.as_call().headers(caller)
    }
}

/// Run `call` through the function route as the caller.
pub async fn invoke(
    function: &FunctionCall,
    caller: &HeaderMap,
    preagg: PreaggCacheCtx,
    runtime: &dyn FunctionRuntime,
) -> Outcome {
    call(&function.as_call(), caller, preagg, runtime).await
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, header};
    use serde_json::Value;

    use super::*;

    fn call() -> FunctionCall {
        FunctionCall {
            org_slug: "acme".into(),
            app_slug: "store-ops".into(),
            function: "ack-alarm".into(),
            notification_id: Uuid::nil(),
            action: "ack".into(),
            link: Some("/alarms/7".into()),
        }
    }

    #[test]
    fn the_function_gets_the_notification_the_action_and_the_link() {
        let body: Value = serde_json::from_slice(&call().body()).unwrap();
        assert_eq!(
            body,
            json!({ "notificationId": Uuid::nil(), "action": "ack", "link": "/alarms/7" })
        );
        assert_eq!(call().uri(), "/customer-apps/acme/store-ops/fn/ack-alarm");
    }

    #[test]
    fn the_callers_credential_goes_and_the_environment_header_does_not() {
        let mut caller = HeaderMap::new();
        caller.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer t"));
        caller.insert("x-oxy-app-env", HeaderValue::from_static("staging"));
        caller.insert("idempotency-key", HeaderValue::from_static("theirs"));
        caller.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
        let h = call().headers(&caller);
        assert_eq!(h[header::AUTHORIZATION], "Bearer t");
        assert!(h.get("x-oxy-app-env").is_none());
        assert_eq!(
            h["idempotency-key"],
            format!("notification-action:{}:ack", Uuid::nil()).as_str()
        );
        assert_eq!(
            h[header::CONTENT_LENGTH],
            call().body().len().to_string().as_str()
        );
        assert_eq!(h[header::CONTENT_TYPE], "application/json");
    }
}
