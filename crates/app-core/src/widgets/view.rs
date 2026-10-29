//! The check of what a widget function returns, before the home sees it.
//!
//! A widget function is app code, so its answer is input: text is trimmed and
//! cut to its limit, lists past their limit are cut, a link that is not
//! app-relative is dropped (no `javascript:`, no off-site or protocol-relative
//! URL ever reaches the page) and a kept one comes back absolute on the app's
//! production URL, and a button naming an action the live build does not
//! declare is dropped. No headline, no view.

use serde::Serialize;
use serde_json::Value;

use super::model::WidgetDef;

pub const MAX_HEADLINE_CHARS: usize = 80;
pub const MAX_DETAIL_CHARS: usize = 120;
pub const MAX_ROW_TITLE_CHARS: usize = 60;
pub const MAX_ROWS: usize = 6;
pub const MAX_BUTTONS: usize = 2;
pub const MAX_BUTTON_LABEL_CHARS: usize = 32;
pub const MAX_CONFIRM_CHARS: usize = 160;
pub const MAX_INPUT_BYTES: usize = 4 * 1024;
pub const MAX_KEY_CHARS: usize = 120;
pub const MAX_LINK_CHARS: usize = 2048;
const MAX_BADGE: u64 = 9_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    #[default]
    Info,
    Attention,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RowState {
    Done,
    Ready,
    Blocked,
    Failed,
    Pending,
}

/// A view the home can draw.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct View {
    pub headline: String,
    pub detail: Option<String>,
    pub level: Level,
    pub progress: Option<Progress>,
    pub rows: Vec<Row>,
    pub buttons: Vec<Button>,
    /// Where "Open" goes: absolute, on the app.
    pub href: Option<String>,
    pub badge: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub title: String,
    pub detail: Option<String>,
    pub state: Option<RowState>,
    pub href: Option<String>,
    pub button: Option<Button>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Button {
    pub action: String,
    pub label: String,
    /// Sent back with the press, as the action's `input`.
    pub input: Value,
    /// What makes two presses the same press; sent back with it.
    pub key: Option<String>,
    /// The question the home asks first; set whenever it must ask.
    pub confirm: Option<String>,
    pub destructive: bool,
}

/// `raw` as a view of `def`, links resolved against `app_url` (the app's
/// production URL). `None` when it has no headline.
pub fn check(raw: &Value, def: &WidgetDef, app_url: &str) -> Option<View> {
    let headline = text(raw.get("headline"), MAX_HEADLINE_CHARS)?;
    let rows = raw
        .get("rows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| row(r, def, app_url))
                .take(MAX_ROWS)
                .collect()
        })
        .unwrap_or_default();
    let buttons = raw
        .get("buttons")
        .and_then(Value::as_array)
        .map(|b| {
            b.iter()
                .filter_map(|b| button(b, def))
                .take(MAX_BUTTONS)
                .collect()
        })
        .unwrap_or_default();
    Some(View {
        headline,
        detail: text(raw.get("detail"), MAX_DETAIL_CHARS),
        level: raw
            .get("level")
            .and_then(Value::as_str)
            .and_then(level)
            .unwrap_or_default(),
        progress: progress(raw.get("progress")),
        rows,
        buttons,
        href: link(raw.get("link"), app_url),
        badge: raw
            .get("badge")
            .and_then(Value::as_u64)
            .map(|n| n.min(MAX_BADGE)),
    })
}

fn row(raw: &Value, def: &WidgetDef, app_url: &str) -> Option<Row> {
    Some(Row {
        title: text(raw.get("title"), MAX_ROW_TITLE_CHARS)?,
        detail: text(raw.get("detail"), MAX_DETAIL_CHARS),
        state: raw.get("state").and_then(Value::as_str).and_then(row_state),
        href: link(raw.get("link"), app_url),
        button: raw.get("button").and_then(|b| button(b, def)),
    })
}

fn button(raw: &Value, def: &WidgetDef) -> Option<Button> {
    let action = def.action(raw.get("action")?.as_str()?)?;
    let label = text(raw.get("label"), MAX_BUTTON_LABEL_CHARS)?;
    let input = raw.get("input").cloned().unwrap_or(Value::Null);
    if input.to_string().len() > MAX_INPUT_BYTES {
        return None;
    }
    let asked = text(raw.get("confirm"), MAX_CONFIRM_CHARS);
    // The manifest's `confirm` is a floor a view cannot lower.
    let confirm = asked.or_else(|| action.confirm.then(|| format!("{label}?")));
    Some(Button {
        action: action.id.clone(),
        label,
        input,
        key: text(raw.get("key"), MAX_KEY_CHARS),
        confirm,
        destructive: action.destructive,
    })
}

fn progress(raw: Option<&Value>) -> Option<Progress> {
    let raw = raw?;
    let total = raw.get("total")?.as_u64().filter(|t| *t > 0)?;
    let done = raw.get("done")?.as_u64()?.min(total);
    Some(Progress { done, total })
}

/// An app-relative link (`/path` or `?query`) made absolute on `app_url`.
/// Anything else, a scheme, a protocol-relative `//host`, a backslash or a
/// control character included, is dropped. So is a dot segment in the path,
/// percent-encoded or not: on a deployment that serves apps on a path route
/// (`/customer-apps/<org>/<app>/`), `/../other-app/` would resolve out of the
/// app, into another app or the product itself.
pub fn link(raw: Option<&Value>, app_url: &str) -> Option<String> {
    let s = raw?.as_str()?.trim();
    let relative = (s.starts_with('/') && !s.starts_with("//")) || s.starts_with('?');
    if !relative
        || s.len() > MAX_LINK_CHARS
        || s.contains('\\')
        || s.chars().any(char::is_control)
        || has_dot_segment(s)
    {
        return None;
    }
    let base = app_url.trim_end_matches('/');
    Some(match s.strip_prefix('?') {
        Some(query) => format!("{base}/?{query}"),
        None => format!("{base}{s}"),
    })
}

/// Whether the path part of `link` (before any `?` or `#`) has a `.` or `..`
/// segment, with `%2e` read as a dot. Conservative: any segment made only of
/// dots is refused.
fn has_dot_segment(link: &str) -> bool {
    let path = link.split(['?', '#']).next().unwrap_or("");
    path.split('/').any(|segment| {
        let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
        !decoded.is_empty() && decoded.chars().all(|c| c == '.')
    })
}

fn text(v: Option<&Value>, max: usize) -> Option<String> {
    let s = v?.as_str()?.trim();
    (!s.is_empty()).then(|| s.chars().take(max).collect())
}

fn level(s: &str) -> Option<Level> {
    match s {
        "ok" => Some(Level::Ok),
        "info" => Some(Level::Info),
        "attention" => Some(Level::Attention),
        "critical" => Some(Level::Critical),
        _ => None,
    }
}

fn row_state(s: &str) -> Option<RowState> {
    match s {
        "done" => Some(RowState::Done),
        "ready" => Some(RowState::Ready),
        "blocked" => Some(RowState::Blocked),
        "failed" => Some(RowState::Failed),
        "pending" => Some(RowState::Pending),
        _ => None,
    }
}
