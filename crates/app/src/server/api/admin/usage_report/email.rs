//! Turns a report into the mail one reader gets.
//!
//! Numbers before words: the headline, a strip of figures, then one line per
//! app worth a look, a table by organization, and the links. Every sentence
//! that was only restating a number is gone — a mail read on a Monday morning
//! is scanned, not read.

use oxy_shared::errors::OxyError;

use crate::emails::EmailMessage;
use crate::emails::usage_report::{EmailHighlight, EmailOrg, EmailStat, ReportEmail, render};

use super::highlights::{Highlight, Tone, highlights};
use super::model::{OrgUsage, Snapshot, Summary, plural};

/// The console pages the mail links to — `ROUTES.ADMIN.USAGE_REPORT` and
/// `ROUTES.ADMIN.SETTINGS` in `web-app/src/libs/utils/routes.ts`.
const REPORT_PATH: &str = "/admin/usage-report";
const SETTINGS_PATH: &str = "/admin/settings";

/// How much of each list a mail carries; the rest is a count and a link.
const ATTENTION_IN_MAIL: usize = 8;
const GOOD_IN_MAIL: usize = 5;
const ORGS_IN_MAIL: usize = 10;
const IDLE_IN_MAIL: usize = 6;
/// Figures to a row in the strip under the headline.
const STATS_PER_ROW: usize = 3;

/// The mail for `snapshot`, which the caller has already narrowed to what its
/// reader may see. `console_url` is the deployment's own address, when known.
pub fn compose(snapshot: &Snapshot, console_url: Option<&str>) -> Result<EmailMessage, OxyError> {
    render(&view(snapshot, console_url))
}

pub(super) fn view(snapshot: &Snapshot, console_url: Option<&str>) -> ReportEmail {
    let summary = snapshot.summary();
    let found = highlights(snapshot);
    let (attention, good, idle) = (
        of_tone(&found, Tone::Attention),
        of_tone(&found, Tone::Good),
        of_tone(&found, Tone::Idle),
    );
    let active: Vec<&OrgUsage> = snapshot
        .orgs
        .iter()
        .filter(|o| o.people > 0 || o.prev_people > 0)
        .collect();
    let host = console_url.and_then(host_of);

    ReportEmail {
        subject: subject(snapshot, &summary, attention.len(), host.as_deref()),
        period: snapshot.period.label(),
        stat_rows: stats(&summary)
            .chunks(STATS_PER_ROW)
            .map(<[EmailStat]>::to_vec)
            .collect(),
        headline: summary.headline,
        attention_more: attention.len().saturating_sub(ATTENTION_IN_MAIL),
        attention: lines(&attention, ATTENTION_IN_MAIL),
        good_more: good.len().saturating_sub(GOOD_IN_MAIL),
        good: lines(&good, GOOD_IN_MAIL),
        orgs_more: active.len().saturating_sub(ORGS_IN_MAIL),
        show_calls: active.iter().any(|o| o.function_calls() > 0),
        show_releases: active.iter().any(|o| o.releases() > 0),
        show_storage: active.iter().any(|o| o.storage_bytes().is_some()),
        orgs: active
            .iter()
            .take(ORGS_IN_MAIL)
            .map(|o| org_line(o))
            .collect(),
        idle: idle_sentence(&idle),
        host,
        report_url: console_url.map(|base| format!("{base}{REPORT_PATH}")),
        settings_url: console_url.map(|base| format!("{base}{SETTINGS_PATH}")),
    }
}

/// `Custom app usage, Sep 28 – Oct 4: 9 people in 2 apps, 1 needs a look (host)`.
fn subject(snapshot: &Snapshot, summary: &Summary, attention: usize, host: Option<&str>) -> String {
    let used = if summary.people == 0 {
        "nobody opened an app".to_string()
    } else {
        format!(
            "{} in {}",
            plural(summary.people, "person", "people"),
            plural(summary.active_apps, "app", "apps")
        )
    };
    let look = match attention {
        0 => String::new(),
        1 => ", 1 needs a look".to_string(),
        n => format!(", {n} need a look"),
    };
    let host = host.map(|h| format!(" ({h})")).unwrap_or_default();
    format!(
        "Custom app usage, {}: {used}{look}{host}",
        snapshot.period.short_label()
    )
}

/// The figures under the headline. Traffic always; the rest only when there is
/// something to say — a `0 releases` cell is a cell nobody needed.
fn stats(summary: &Summary) -> Vec<EmailStat> {
    let stat = |label: &str, value: String, note: String| EmailStat {
        label: label.to_string(),
        value,
        note,
    };
    let mut all = vec![
        stat(
            "People",
            grouped(summary.people),
            change(summary.people, summary.prev_people),
        ),
        stat(
            "Opens",
            grouped(summary.views),
            change(summary.views, summary.prev_views),
        ),
        stat(
            "Apps in use",
            format!("{} of {}", summary.active_apps, summary.apps),
            String::new(),
        ),
    ];
    if summary.function_calls > 0 {
        let failed = match summary.function_failures {
            0 => String::new(),
            n => format!("{} failed", grouped(n)),
        };
        all.push(stat(
            "Function calls",
            grouped(summary.function_calls),
            failed,
        ));
    }
    if summary.releases > 0 {
        all.push(stat("Releases", grouped(summary.releases), String::new()));
    }
    if let Some(bytes) = summary.storage_bytes {
        let moved = summary
            .storage_bytes_before
            .map_or_else(String::new, |before| bytes_change(bytes, before));
        all.push(stat("Storage", bytes_label(bytes), moved));
    }
    all
}

fn of_tone(found: &[Highlight], tone: Tone) -> Vec<&Highlight> {
    found.iter().filter(|h| h.tone == tone).collect()
}

fn lines(found: &[&Highlight], keep: usize) -> Vec<EmailHighlight> {
    found
        .iter()
        .take(keep)
        .map(|h| EmailHighlight {
            app: h.app_name.clone(),
            org: h.org_name.clone(),
            label: h.label.to_string(),
            detail: h.detail.clone(),
        })
        .collect()
}

fn org_line(org: &OrgUsage) -> EmailOrg {
    EmailOrg {
        name: org.name.clone(),
        people: org.people,
        change: change(org.people, org.prev_people),
        views: grouped(org.views()),
        calls: grouped(org.function_calls()),
        releases: grouped(org.releases()),
        // Unmeasured is not empty: a dash, not `0 B`.
        storage: org
            .storage_bytes()
            .map_or_else(|| "–".to_string(), bytes_label),
    }
}

/// `+3`, `−2`; nothing for no change.
fn change(now: u64, before: u64) -> String {
    match now.cmp(&before) {
        std::cmp::Ordering::Greater => format!("+{}", grouped(now - before)),
        std::cmp::Ordering::Less => format!("−{}", grouped(before - now)),
        std::cmp::Ordering::Equal => String::new(),
    }
}

/// `+120 MB`, `−1.2 GB`; nothing for no change.
fn bytes_change(now: u64, before: u64) -> String {
    match now.cmp(&before) {
        std::cmp::Ordering::Greater => format!("+{}", bytes_label(now - before)),
        std::cmp::Ordering::Less => format!("−{}", bytes_label(before - now)),
        std::cmp::Ordering::Equal => String::new(),
    }
}

/// `1,240`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `512 B`, `12 KB`, `3.4 MB`, `640 MB`, `1.2 GB` — binary units. MB and GB
/// carry one decimal under ten, where it says something, and none above.
fn bytes_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    let in_unit = |size: f64, unit: &str| {
        if size < 10.0 {
            format!("{size:.1} {unit}")
        } else {
            format!("{size:.0} {unit}")
        }
    };
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else if b < KB * KB * KB {
        in_unit(b / (KB * KB), "MB")
    } else {
        in_unit(b / (KB * KB * KB), "GB")
    }
}

/// `Checklists (Poke House), Archive (Rivermark) and 3 more.`
fn idle_sentence(idle: &[&Highlight]) -> String {
    if idle.is_empty() {
        return String::new();
    }
    let named: Vec<String> = idle
        .iter()
        .take(IDLE_IN_MAIL)
        .map(|h| format!("{} ({})", h.app_name, h.org_name))
        .collect();
    match idle.len().saturating_sub(IDLE_IN_MAIL) {
        0 => format!("{}.", named.join(", ")),
        more => format!("{} and {more} more.", named.join(", ")),
    }
}

/// `app.oxygen-hq.com` from `https://app.oxygen-hq.com`.
fn host_of(url: &str) -> Option<String> {
    let host = url.split_once("://")?.1.split('/').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
