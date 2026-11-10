//! `oxy.cli_approval`: `oxyc … --no-browser --approver <email>` asks this
//! person to approve a device-code request.
//!
//! Called by the start of the device-code flow
//! (`user_tokens/device_login.rs`) once the request is stored and its named
//! approver resolved to an active user. The notice reaches the person on push,
//! email, Slack and in the inbox, so an account owner can approve from their
//! phone without anyone handing them a URL.
//!
//! **The notice never holds the code.** Its subject is the request's user code
//! (to find the row it is worded from), but the words say only to type "the
//! code from the terminal that ran oxyc", and the link is the page without a
//! `?code=`. Seeing the code is the check that the person is at, or was told
//! by, the terminal that asked; a notice that carried it would let the inbox
//! approve what nobody at the terminal sees.

use chrono::{DateTime, Utc};
use entity::cli_device_requests::{self, Column};
use oxy_auth::token::device_login;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use super::render::Rendered;
use super::subjects::Record;

/// The write: `oxy_app_core`'s, because the device-code flow that calls it is
/// not this surface's.
pub use oxy_app_core::notifications::platform::cli_approval::notify_cli_approval;

/// The page a person approves on. No `?code=`: they type it.
pub const PAGE: &str = "/cli-auth/device";
/// The longest title a channel is given.
const TITLE_MAX_CHARS: usize = 200;

/// The requests `user_codes` name, as the notice is worded from them. A
/// request already swept is not returned; its notice reads as the plain
/// fallback sentence.
pub async fn load(
    db: &DatabaseConnection,
    user_codes: Vec<String>,
) -> Result<Vec<(String, Record)>, DbErr> {
    if user_codes.is_empty() {
        return Ok(Vec::new());
    }
    Ok(cli_device_requests::Entity::find()
        .filter(Column::UserCode.is_in(user_codes))
        .all(db)
        .await?
        .into_iter()
        .filter_map(record)
        .collect())
}

/// `login`, `agent` or `sandbox_agent`, from the ask's `kind`. `None` for a
/// kind this binary does not know.
fn kind_of(ask: &Value) -> Option<&'static str> {
    match ask.get("kind").and_then(Value::as_str) {
        None => Some("login"),
        Some("agent") => Some("agent"),
        Some("sandbox_agent") => Some("sandbox_agent"),
        Some(_) => None,
    }
}

fn record(row: cli_device_requests::Model) -> Option<(String, Record)> {
    let text = |key: &str| row.ask.get(key).and_then(Value::as_str);
    let apps = text("apps")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|app| !app.is_empty())
        .map(str::to_string)
        .collect();
    let hours = text("hours").and_then(|h| h.trim().parse::<i64>().ok());
    let record = Record::CliDeviceRequest {
        user_code: row.user_code.clone(),
        hostname: row.hostname.clone(),
        kind: kind_of(&row.ask)?,
        apps,
        hours,
        status: row.status.clone(),
        expires_at: row.expires_at.with_timezone(&Utc),
    };
    Some((row.user_code, record))
}

/// The longest label a title carries for a host or an app.
const LABEL_MAX_CHARS: usize = 64;

/// Text an asker chose, as a title may carry it: letters, digits, `.` and `-`
/// (and `/` for an `org/app` reference), at most 64, and never a run that
/// reads as a user code. The CLI posts `os.hostname()` and app references as
/// they are and the start accepts any printable text, but this text is
/// pushed, mailed and DM'd at an anonymous caller's choosing: anything else
/// becomes `-`, and text that could smuggle a code is dropped altogether.
fn label(text: &str, slash: bool) -> Option<String> {
    let mut label = String::new();
    for c in text.chars() {
        let next = if c.is_ascii_alphanumeric() || c == '.' || c == '-' || (slash && c == '/') {
            c
        } else {
            '-'
        };
        if next == '-' && label.ends_with('-') {
            continue;
        }
        label.push(next);
        if label.len() >= LABEL_MAX_CHARS {
            break;
        }
    }
    let label = label.trim_matches(|c| c == '-' || c == '.' || c == '/');
    (!label.is_empty() && !device_login::carries_user_code(label)).then(|| label.to_string())
}

/// What the request asks for, in a title's words. The hostname and the app
/// references are the asker's; each is a label, and the assembled title is
/// checked once more, since a code can be split across two labels and the
/// `, ` between them. A title that could carry one names no host and no app.
fn title(kind: &str, hostname: &str, apps: &[String], hours: Option<i64>) -> String {
    let host = label(hostname, false).unwrap_or_else(|| "a machine".to_string());
    let apps: Vec<String> = apps.iter().filter_map(|a| label(a, true)).collect();
    let worded = wording(kind, &host, &apps, hours);
    if device_login::carries_user_code(&worded) {
        return truncate(wording(kind, "a machine", &[], hours));
    }
    truncate(worded)
}

fn wording(kind: &str, hostname: &str, apps: &[String], hours: Option<i64>) -> String {
    let h = hours.map(|h| format!(" ({h} h)")).unwrap_or_default();
    match kind {
        "agent" => format!("oxyc on {hostname} asks for an agent token{h}"),
        "sandbox_agent" if apps.is_empty() => {
            format!("oxyc on {hostname} asks for a sandbox agent token{h}")
        }
        "sandbox_agent" => format!(
            "oxyc on {hostname} asks for a sandbox agent token for {}{h}",
            apps.join(", ")
        ),
        _ => format!("oxyc on {hostname} asks to sign in as you"),
    }
}

/// At most [`TITLE_MAX_CHARS`] characters, ending in `…` when cut.
fn truncate(title: String) -> String {
    if title.chars().count() <= TITLE_MAX_CHARS {
        return title;
    }
    let mut cut: String = title.chars().take(TITLE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The words for a request: what it asks, then — while it waits — how to
/// answer it, or — once decided or lapsed — how it ended. The title stays the
/// same either way, so the inbox row reads the same after it is acted on.
pub fn words(record: &Record, now: DateTime<Utc>) -> Option<Rendered> {
    let Record::CliDeviceRequest {
        hostname,
        kind,
        apps,
        hours,
        status,
        expires_at,
        ..
    } = record
    else {
        return None;
    };
    let body = match status.as_str() {
        "approved" => "Approved.".to_string(),
        "denied" => "Declined.".to_string(),
        _ if *expires_at <= now => "Expired.".to_string(),
        _ => format!(
            "Open the approval page and type the code from the terminal that ran oxyc. \
             If you did not run this command, ignore it: nothing happens unless the code is \
             typed. Expires at {}.",
            expires_at.format("%H:%M UTC")
        ),
    };
    Some(Rendered {
        title: title(kind, hostname, apps, *hours),
        body: Some(body),
        href: Some(PAGE.to_string()),
    })
}

#[cfg(test)]
#[path = "cli_approval_tests.rs"]
mod tests;
