//! A person's light/dark mode: the one appearance setting Oxygen and every
//! custom app follow (decision 19, 16 Nov: every app has both modes and follows
//! Oxygen home; a change in an app changes Oxygen too).
//!
//! - `GET`/`PUT /api/me/appearance` — Oxygen web's read and write, on the
//!   authenticated tree ([`routes::get_mine`], [`routes::put_mine`]).
//! - `GET`/`PUT /api/projects/{project_id}/shell-context/mode` — the same
//!   value for a bundle, behind the data-plane gate chain the shell context
//!   rides, so it works on every host an app is served from: the admin host's
//!   path URL, an org subdomain, a custom-app subdomain and a custom domain
//!   ([`routes::get_for_app`], [`routes::put_for_app`]).
//! - `GET /api/projects/{project_id}/shell-context` carries it as `mode`
//!   (`custom_apps_shell_context`), so an app paints from it on boot.
//!
//! Stored per person across orgs in `user_ui_preferences` (control plane: a
//! choice about Oxy's own interface, with no org content in it; see
//! `entity::user_ui_preferences`). No row is "not chosen", answered as
//! `"mode": null`, and the caller keeps Oxygen's default (light).
//!
//! A frontline (PIN) identity has no Oxygen account to carry a mode: every
//! route answers it 404, and the shell context leaves `mode` out for it, the
//! same absence an older server gives. Every answer is per person, so every
//! one is `Cache-Control: private, no-store`.

pub mod routes;
pub mod store;

use serde_json::{Value, json};

/// One of the three modes a person can pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Light,
    Dark,
    /// Follow the device's own light/dark setting.
    System,
}

impl ColorMode {
    /// The stored and wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    /// The mode a stored or sent string names; `None` for anything else.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// What every read and write answers: `{ "mode": "dark" }`, or
/// `{ "mode": null }` for a person who has not chosen.
pub fn answer(mode: Option<ColorMode>) -> Value {
    json!({ "mode": mode.map(ColorMode::as_str) })
}

/// A write's body, checked: `{ "mode": "light" | "dark" | "system" }`.
pub fn parse_body(body: &Value) -> Result<ColorMode, &'static str> {
    body.get("mode")
        .and_then(Value::as_str)
        .and_then(ColorMode::parse)
        .ok_or("send { \"mode\": \"light\" | \"dark\" | \"system\" }")
}

/// Whether a signed-in identity has an Oxygen account that carries a mode. A
/// frontline worker (no mailbox, `email` is `None`) does not.
pub fn has_mode(user: &oxy_auth::types::AuthenticatedUser) -> bool {
    user.email.is_some()
}

#[cfg(test)]
mod tests;
