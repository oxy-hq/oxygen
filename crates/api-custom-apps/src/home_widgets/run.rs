//! Running a widget's function for its view, and a button's action.
//!
//! Both go through the app's own function route as the caller
//! (`oxy_app_core::function_answer`), so every gate a direct call would meet
//! applies. A press carries an idempotency key built from the widget, the
//! action and the button's `key` (or a hash of its input), so a double click,
//! a second tab or a stale card replays the first answer instead of doing the
//! work twice.

use std::time::Duration;

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use oxy_app_core::function_answer::{Call, Outcome, call};
use oxy_app_core::function_seam::FunctionRuntime;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_app_core::widgets::model::{ActionDef, WidgetDef};
use oxy_app_core::widgets::view::{self, View};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::load::WidgetApp;

/// How long one widget may take to answer before the home stops waiting.
pub const VIEW_TIMEOUT: Duration = Duration::from_secs(8);
/// The longest `message` an action's answer may put on the card.
const MAX_MESSAGE_CHARS: usize = 160;

/// One widget as the home gets it: its view, or why there is none.
#[derive(Debug, Serialize)]
pub struct WidgetCard {
    pub app: WidgetApp,
    pub id: String,
    pub title: String,
    pub size: oxy_app_core::widgets::model::Size,
    pub view: Option<View>,
    pub error: Option<String>,
}

/// What the caller's request carries into each call.
pub struct Caller<'a> {
    pub headers: &'a HeaderMap,
    pub preagg: PreaggCacheCtx,
    pub runtime: &'a dyn FunctionRuntime,
}

/// Run `def`'s function and check its view.
pub async fn read(app: &WidgetApp, def: &WidgetDef, caller: &Caller<'_>) -> WidgetCard {
    let body = Bytes::from(json!({ "widget": def.id }).to_string());
    let function = Call {
        org_slug: &app.org_slug,
        app_slug: &app.slug,
        function: &def.function,
        body,
        idempotency_key: None,
    };
    let answered = tokio::time::timeout(
        VIEW_TIMEOUT,
        call(
            &function,
            caller.headers,
            caller.preagg.clone(),
            caller.runtime,
        ),
    )
    .await;
    let (view, error) = match answered {
        Err(_) => (
            None,
            Some("This widget took too long to answer.".to_string()),
        ),
        Ok(Outcome::Done(raw)) => match view::check(&raw, def, &app.url) {
            Some(v) => (Some(v), None),
            None => (None, Some("This widget returned nothing to show.".into())),
        },
        Ok(outcome) => (None, Some(read_failure(&outcome))),
    };
    WidgetCard {
        app: app.clone(),
        id: def.id.clone(),
        title: def.title.clone(),
        size: def.size,
        view,
        error,
    }
}

fn read_failure(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Refused(StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _) => {
            "You can't run this widget.".into()
        }
        Outcome::Refused(status, body) => match body["message"].as_str() {
            Some(said) => format!("This widget couldn't load: {said}"),
            None => format!("This widget couldn't load ({}).", status.as_u16()),
        },
        Outcome::Failed(message) => format!("This widget failed: {message}"),
        Outcome::Done(_) => unreachable!("a success has a view or says it has none"),
    }
}

/// A button press, checked.
pub struct Press<'a> {
    pub widget: &'a WidgetDef,
    pub action: &'a ActionDef,
    pub input: Value,
    pub key: Option<String>,
}

impl Press<'_> {
    /// What the action's function receives.
    pub fn body(&self) -> Bytes {
        Bytes::from(
            json!({ "widget": self.widget.id, "action": self.action.id, "input": self.input })
                .to_string(),
        )
    }

    /// What makes two presses the same press: the button's own key, else
    /// its input. Hashed either way: the key is app text (`Café-2026-09`), and
    /// the function route reads the header only as visible ASCII, so a raw
    /// key with an accent or a control character would arrive as no key at
    /// all and the press would run unguarded. Widget and action ids are
    /// ASCII by the manifest check, so the whole key is header-safe and well
    /// under the route's 200-character cap.
    pub fn idempotency_key(&self) -> String {
        let what = match &self.key {
            Some(key) => format!("k-{}", digest(key)),
            None => format!("in-{}", digest(&self.input.to_string())),
        };
        format!("widget:{}:{}:{what}", self.widget.id, self.action.id)
    }
}

/// The first 128 bits of `text`'s SHA-256, as hex.
fn digest(text: &str) -> String {
    hex::encode(&Sha256::digest(text.as_bytes())[..16])
}

/// What a press answers.
#[derive(Debug, PartialEq)]
pub enum Pressed {
    Done {
        message: Option<String>,
        view: Option<View>,
    },
    Failed {
        status: StatusCode,
        error: String,
        message: String,
    },
}

/// Run `press` as the caller.
pub async fn press(app: &WidgetApp, press: &Press<'_>, caller: &Caller<'_>) -> Pressed {
    let function = Call {
        org_slug: &app.org_slug,
        app_slug: &app.slug,
        function: &press.action.function,
        body: press.body(),
        idempotency_key: Some(press.idempotency_key()),
    };
    let outcome = call(
        &function,
        caller.headers,
        caller.preagg.clone(),
        caller.runtime,
    )
    .await;
    pressed(outcome, press.widget, &app.url)
}

/// The answer for `outcome`. A `401` or `403` from the function route is
/// `403 FunctionRefused`, never a `401`: this route already authenticated the
/// caller, and the web app signs a person out on any `401`.
pub fn pressed(outcome: Outcome, widget: &WidgetDef, app_url: &str) -> Pressed {
    match outcome {
        Outcome::Done(answer) => Pressed::Done {
            message: answer["message"]
                .as_str()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|m| m.chars().take(MAX_MESSAGE_CHARS).collect()),
            view: answer
                .get("view")
                .and_then(|v| view::check(v, widget, app_url)),
        },
        Outcome::Refused(status, body)
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) =>
        {
            Pressed::Failed {
                status: StatusCode::FORBIDDEN,
                error: "FunctionRefused".into(),
                message: match (status, body["message"].as_str()) {
                    (StatusCode::FORBIDDEN, Some(said)) => said.to_string(),
                    _ => "You can't run this app's action.".into(),
                },
            }
        }
        Outcome::Refused(status, body) => Pressed::Failed {
            status,
            error: body["error"]
                .as_str()
                .unwrap_or("FunctionRefused")
                .to_string(),
            message: body["message"]
                .as_str()
                .unwrap_or("The app refused the action.")
                .to_string(),
        },
        Outcome::Failed(message) => Pressed::Failed {
            status: StatusCode::BAD_GATEWAY,
            error: "FunctionFailed".into(),
            message,
        },
    }
}
