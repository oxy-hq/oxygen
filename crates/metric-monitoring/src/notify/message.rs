//! The Slack message for a set of newly claimed events. Pure: rows in, blocks
//! out — no database, no network, so what a channel will read is unit-tested.
//!
//! One message per scan, however many events it found. An event is one line,
//! and events that fired together as a cohort (one moment across segments)
//! share a line: a chain-wide drop across eighty stores is one sentence, not
//! eighty.

use chrono::{DateTime, FixedOffset, Utc};
use sea_orm::FromQueryResult;
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

use crate::detect::severity_rank;

#[cfg(test)]
mod tests;
mod text;

use text::{escape, format_value, truncate};

/// Lines a message lists before it says "and N more".
pub const MAX_LINES: usize = 10;
/// A label or segment longer than this is cut: both are tenant text, and one
/// long dimension value must not push the rest of the message out.
const MAX_NAME_CHARS: usize = 80;
/// Slack refuses a section whose text passes 3000 characters.
const MAX_SECTION_CHARS: usize = 2900;

/// One flagged bucket of an event being announced — the columns a message
/// reads, and nothing else.
#[derive(Debug, Clone, PartialEq, FromQueryResult)]
pub struct Bucket {
    pub event_id: Uuid,
    pub measure: String,
    pub label: Option<String>,
    pub granularity: String,
    pub period_start: DateTime<FixedOffset>,
    pub observed: f64,
    pub expected: f64,
    pub z_score: f64,
    pub severity: String,
    pub dimension_key: String,
    pub cohort_id: Option<Uuid>,
    pub cohort_label: Option<String>,
}

/// A message ready for `chat.postMessage`.
#[derive(Debug, Clone, PartialEq)]
pub struct SlackMessage {
    /// The notification / screen-reader fallback.
    pub text: String,
    pub blocks: Value,
    /// How many events the message announces, counting those behind "more".
    pub events: usize,
}

/// One event, rolled up the way its Insights Inbox row is.
struct Event<'a> {
    /// The bucket furthest from expectation: max `|z|`, earliest wins a tie.
    peak: &'a Bucket,
    first: DateTime<FixedOffset>,
    buckets: usize,
    /// Max over the buckets, not the peak's — a slide files its later days
    /// `low`, and the peak by `|z|` can be one of them.
    severity: u8,
}

/// What one line of the message stands for.
enum Line<'a> {
    One(Event<'a>),
    /// Two or more events whose peaks fired in the same cohort.
    Cohort(Vec<Event<'a>>),
}

/// Build the message for `buckets`, or `None` when there is nothing to say.
///
/// `inbox_url` is the workspace's Insights Inbox; without one the message
/// simply carries no link.
pub fn build(
    workspace_name: &str,
    inbox_url: Option<&str>,
    buckets: &[Bucket],
) -> Option<SlackMessage> {
    let events = roll_up(buckets);
    if events.is_empty() {
        return None;
    }
    let total = events.len();
    let noun = if total == 1 { "insight" } else { "insights" };
    let name = escape(&truncate(workspace_name));
    let text = format!("{total} new {noun} in {name}");

    let lines = into_lines(events);
    let (shown, body) = render_lines(&lines);
    let hidden = total - lines[..shown].iter().map(Line::events).sum::<usize>();

    let mut blocks = vec![
        section(format!("*{total} new {noun}* in *{name}*")),
        section(body),
    ];
    let mut footer = Vec::new();
    if hidden > 0 {
        footer.push(format!("…and {hidden} more"));
    }
    if let Some(url) = inbox_url {
        footer.push(format!("<{url}|Open the Insights Inbox>"));
    }
    if !footer.is_empty() {
        blocks.push(json!({
            "type": "context",
            "elements": [{ "type": "mrkdwn", "text": footer.join(" · ") }],
        }));
    }
    Some(SlackMessage {
        text,
        blocks: Value::Array(blocks),
        events: total,
    })
}

fn section(text: String) -> Value {
    json!({ "type": "section", "text": { "type": "mrkdwn", "text": text } })
}

fn roll_up(buckets: &[Bucket]) -> Vec<Event<'_>> {
    let mut by_event: HashMap<Uuid, Vec<&Bucket>> = HashMap::new();
    for bucket in buckets {
        by_event.entry(bucket.event_id).or_default().push(bucket);
    }
    by_event
        .into_values()
        .map(|mut rows| {
            rows.sort_by_key(|b| b.period_start);
            let peak = rows.iter().copied().fold(rows[0], |best, b| {
                if b.z_score.abs() > best.z_score.abs() {
                    b
                } else {
                    best
                }
            });
            Event {
                peak,
                first: rows[0].period_start,
                buckets: rows.len(),
                severity: rows
                    .iter()
                    .map(|b| severity_rank(&b.severity))
                    .max()
                    .unwrap_or(0),
            }
        })
        .collect()
}

/// Fold cohort members together and put the lines in reading order: most
/// severe first, then furthest from expectation. Every key is total, down to
/// the event id, so the same events always produce the same message.
fn into_lines(events: Vec<Event<'_>>) -> Vec<Line<'_>> {
    let mut cohorts: HashMap<Uuid, Vec<Event<'_>>> = HashMap::new();
    let mut lines = Vec::new();
    for event in events {
        match event.peak.cohort_id {
            Some(id) => cohorts.entry(id).or_default().push(event),
            None => lines.push(Line::One(event)),
        }
    }
    for mut members in cohorts.into_values() {
        if members.len() == 1 {
            lines.extend(members.pop().map(Line::One));
        } else {
            members.sort_by_key(|e| e.peak.event_id);
            lines.push(Line::Cohort(members));
        }
    }
    lines.sort_by(|a, b| {
        b.severity()
            .cmp(&a.severity())
            .then(b.magnitude().total_cmp(&a.magnitude()))
            .then(a.lead().peak.event_id.cmp(&b.lead().peak.event_id))
    });
    lines
}

/// Render as many of the first [`MAX_LINES`] lines as fit one Slack section.
/// Returns how many were rendered alongside the text.
fn render_lines(lines: &[Line<'_>]) -> (usize, String) {
    let mut rendered: Vec<String> = lines.iter().take(MAX_LINES).map(Line::render).collect();
    while rendered.len() > 1 && joined_len(&rendered) > MAX_SECTION_CHARS {
        rendered.pop();
    }
    (rendered.len(), rendered.join("\n"))
}

fn joined_len(lines: &[String]) -> usize {
    lines.iter().map(|l| l.chars().count() + 1).sum()
}

impl<'a> Line<'a> {
    fn lead(&self) -> &Event<'a> {
        match self {
            Line::One(event) => event,
            Line::Cohort(members) => &members[0],
        }
    }

    fn events(&self) -> usize {
        match self {
            Line::One(_) => 1,
            Line::Cohort(members) => members.len(),
        }
    }

    fn severity(&self) -> u8 {
        match self {
            Line::One(event) => event.severity,
            Line::Cohort(members) => members.iter().map(|e| e.severity).max().unwrap_or(0),
        }
    }

    /// How far from expectation, as a fraction. A breach past an expectation of
    /// zero has no ratio and sorts first — it is unbounded, not unknown.
    fn magnitude(&self) -> f64 {
        let of = |e: &Event<'_>| deviation(e.peak).map_or(f64::INFINITY, f64::abs);
        match self {
            Line::One(event) => of(event),
            Line::Cohort(members) => members.iter().map(of).fold(0.0, f64::max),
        }
    }

    fn render(&self) -> String {
        let peak = self.lead().peak;
        let title = escape(&truncate(peak.label.as_deref().unwrap_or(&peak.measure)));
        let head = format!("{} *{title}*", severity_mark(self.severity()));
        let day = cohort_day(peak);
        match self {
            Line::Cohort(members) => format!(
                "{head} — {} expected across {} segments {}{day}",
                direction(peak),
                members.len(),
                when(peak),
            ),
            Line::One(event) => {
                let segment = match segment_label(&peak.dimension_key) {
                    Some(s) => format!(" · {}", escape(&truncate(&s))),
                    None => String::new(),
                };
                format!("{head}{segment} — {}{}{day}", describe(peak), since(event))
            }
        }
    }
}

/// `23.4% below expected on 2026-10-04 (1,204 vs 1,571)`.
fn describe(peak: &Bucket) -> String {
    let by = match deviation(peak) {
        Some(d) => format!("{:.1}% ", d.abs() * 100.0),
        None => String::new(),
    };
    format!(
        "{by}{} expected {} ({} vs {})",
        direction(peak),
        when(peak),
        format_value(peak.observed),
        format_value(peak.expected),
    )
}

/// The tail a multi-bucket event adds: ` · 3 days flagged since 2026-10-02`.
fn since(event: &Event<'_>) -> String {
    if event.buckets < 2 {
        return String::new();
    }
    let grain = match event.peak.granularity.as_str() {
        "day" => "days",
        "week" => "weeks",
        "month" => "months",
        _ => "buckets",
    };
    format!(
        " · {} {grain} flagged since {}",
        event.buckets,
        period_label(&event.peak.granularity, event.first)
    )
}

fn cohort_day(peak: &Bucket) -> String {
    match peak.cohort_label.as_deref() {
        Some(label) => format!(" · {}", escape(&truncate(label))),
        None => String::new(),
    }
}

fn direction(bucket: &Bucket) -> &'static str {
    if bucket.observed < bucket.expected {
        "below"
    } else {
        "above"
    }
}

/// `(observed − expected) / |expected|`, the Insights Inbox's own number, and
/// `None` on the same condition it shows a dash for.
fn deviation(bucket: &Bucket) -> Option<f64> {
    (bucket.expected.abs() >= 1e-9)
        .then(|| (bucket.observed - bucket.expected) / bucket.expected.abs())
        .filter(|d| d.is_finite())
}

/// `on 2026-10-04`, `in the week of 2026-09-28`, `in Sep 2026`.
fn when(bucket: &Bucket) -> String {
    let label = period_label(&bucket.granularity, bucket.period_start);
    match bucket.granularity.as_str() {
        "week" | "month" => format!("in {label}"),
        _ => format!("on {label}"),
    }
}

/// A bucket's name, read the way the Insights Inbox reads it — off the UTC
/// date of `period_start` — so the line and the row it links to name the same
/// day.
fn period_label(granularity: &str, period_start: DateTime<FixedOffset>) -> String {
    let start = period_start.with_timezone(&Utc);
    match granularity {
        "week" => format!("the week of {}", start.format("%Y-%m-%d")),
        "month" => start.format("%b %Y").to_string(),
        _ => start.format("%Y-%m-%d").to_string(),
    }
}

/// `sales_daily.restaurant_id=loc-abc;sales_daily.channel=web` →
/// `restaurant_id=loc-abc, channel=web`. `None` for a chain-wide monitor.
fn segment_label(dimension_key: &str) -> Option<String> {
    let parts: Vec<String> = dimension_key
        .split(';')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((member, values)) => {
                format!("{}={values}", member.rsplit('.').next().unwrap_or(member))
            }
            None => pair.to_string(),
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn severity_mark(rank: u8) -> &'static str {
    match rank {
        2 => "🔴",
        1 => "🟠",
        _ => "🟡",
    }
}
