//! Turning tenant text and raw numbers into something safe and short enough
//! to put in a Slack line.

use super::MAX_NAME_CHARS;

/// Slack's three control characters. Every label, segment and workspace name
/// is tenant text, and an unescaped `<!channel>` in a dimension value would
/// page the whole channel.
pub(super) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_NAME_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_NAME_CHARS - 1).collect();
    format!("{cut}…")
}

/// A value at a glance: whole and grouped from a thousand up, a few decimals
/// below it. The measure's own format is not on the row, so no unit is shown.
pub(super) fn format_value(value: f64) -> String {
    if !value.is_finite() {
        return "n/a".to_string();
    }
    let abs = value.abs();
    // On the rounded value, so 999.6 is `1,000` and not an ungrouped `1000`.
    if abs.round() >= 1000.0 {
        return group_thousands(value.round());
    }
    let decimals = if abs >= 100.0 {
        0
    } else if abs >= 1.0 {
        2
    } else {
        4
    };
    let fixed = format!("{value:.decimals$}");
    let trimmed = if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.')
    } else {
        fixed.as_str()
    };
    // `-0.00004` rounds to `-0.0000`; a signed zero is not a number to show.
    if trimmed == "-0" { "0" } else { trimmed }.to_string()
}

fn group_thousands(whole: f64) -> String {
    let digits = format!("{:.0}", whole.abs());
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if whole < 0.0 { format!("-{out}") } else { out }
}
