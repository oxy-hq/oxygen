use chrono::{DateTime, FixedOffset, Utc};
use uuid::Uuid;

use super::evaluator::{DimensionFailure, HealthStatus};
use super::queries::WorkspaceLabel;
use oxy_shared::errors::OxyError;
use oxy_slack_client::SlackClient;

/// Default gap between reminders for a workspace that stays unhealthy.
const DEFAULT_REALERT_HOURS: i64 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertDecision {
    /// No notification.
    Silent,
    /// Transitioned into unhealthy — push an alert.
    Alert,
    /// Still unhealthy. `escalated` distinguishes an immediate re-page because the
    /// failure set grew or got worse from the plain periodic reminder.
    Reminder { escalated: bool },
    /// Returned to healthy from a worse state — push a recovery note.
    Recovery,
}

/// Everything the transition diff needs. Grouped because "is this worth a Slack
/// message?" now depends on three things beyond the status pair: what we last
/// told Slack, when we told it, and how often we're willing to repeat.
pub struct AlertInput<'a> {
    /// Status recorded by the previous eval pass.
    pub prev: Option<HealthStatus>,
    pub next: HealthStatus,
    /// The failure set carried by the *last alert we pushed* — not the previous
    /// pass's. Comparing against what Slack was actually told keeps a failure set
    /// that flaps between passes from paging on every flap.
    pub alerted_failures: Option<&'a [DimensionFailure]>,
    pub next_failures: &'a [DimensionFailure],
    /// When Slack was last paged about this workspace. `None` means never — an
    /// unhealthy workspace in that state is due for a reminder immediately, which
    /// is what backfills alerting for rows written before this existed.
    pub last_alerted_at: Option<DateTime<FixedOffset>>,
    pub now: DateTime<Utc>,
    /// Gap between reminders; `None` disables them (transition-only alerting).
    pub reminder_after: Option<chrono::Duration>,
}

/// Alert on transition INTO unhealthy, repeat on a cadence for as long as it
/// stays unhealthy, and push one recovery note on return to healthy. Degraded is
/// dashboard-only — never pages Slack.
///
/// The repeat is the point: an outage that persists for days used to produce a
/// single message that scrolled out of the channel, after which the workspace was
/// indistinguishable from a healthy one.
pub fn decide_transition(input: &AlertInput<'_>) -> AlertDecision {
    match (input.prev, input.next) {
        (_, HealthStatus::Unhealthy) if input.prev != Some(HealthStatus::Unhealthy) => {
            AlertDecision::Alert
        }
        (Some(HealthStatus::Unhealthy), HealthStatus::Unhealthy) => still_unhealthy(input),
        (Some(p), HealthStatus::Healthy) if p != HealthStatus::Healthy => AlertDecision::Recovery,
        _ => AlertDecision::Silent,
    }
}

/// The re-alert arm: an escalated failure set pages now, otherwise wait out the
/// reminder interval.
///
/// `reminder_after == None` is checked first, so `OXY_HEALTH_REALERT_HOURS=0`
/// really is transition-only alerting as documented — with repeats switched off,
/// nothing about a workspace that is *already* unhealthy pages again.
fn still_unhealthy(input: &AlertInput<'_>) -> AlertDecision {
    let Some(after) = input.reminder_after else {
        return AlertDecision::Silent;
    };
    if escalated(input.alerted_failures, input.next_failures) {
        return AlertDecision::Reminder { escalated: true };
    }
    // Never alerted (a pre-existing row, or every previous push failed) reads as
    // "overdue" — one message now beats staying silent about a live outage.
    let due = match input.last_alerted_at {
        Some(at) => input.now.signed_duration_since(at.with_timezone(&Utc)) >= after,
        None => true,
    };
    if due {
        AlertDecision::Reminder { escalated: false }
    } else {
        AlertDecision::Silent
    }
}

/// Did the workspace pick up a failure Slack has not been told about — a
/// dimension that wasn't failing before, or one that got worse (degraded →
/// unhealthy)?
///
/// Only escalation counts. A dimension that *recovers* or drops back to degraded
/// is not worth an out-of-band page: it makes the next reminder shorter, not the
/// current situation newer. And this compares dimensions, never reason text —
/// reason strings carry live counts that drift on nearly every pass, so diffing
/// them would page every ~10 minutes and bypass the reminder interval entirely.
///
/// `None` (never alerted) is not an escalation: the reminder clock, which reads
/// "never alerted" as overdue, decides that case.
fn escalated(alerted: Option<&[DimensionFailure]>, next: &[DimensionFailure]) -> bool {
    let Some(alerted) = alerted else {
        return false;
    };
    next.iter().any(|n| {
        match alerted.iter().find(|a| a.dimension == n.dimension) {
            // Newly failing dimension.
            None => true,
            // Same dimension, worse than what we paged about.
            Some(a) => n.status > a.status,
        }
    })
}

/// Reminder cadence from env. `OXY_HEALTH_REALERT_HOURS=0` (or negative) turns
/// reminders off and restores transition-only alerting.
pub fn reminder_interval() -> Option<chrono::Duration> {
    let hours = std::env::var("OXY_HEALTH_REALERT_HOURS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_REALERT_HOURS);
    (hours > 0).then(|| chrono::Duration::hours(hours))
}

/// Slack mrkdwn reserves these three characters; workspace and org names are
/// tenant-controlled free text, so they get escaped before interpolation.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Deep link to this workspace's admin Health tab. `None` when `OXY_API_URL` is
/// unset or malformed (a cluster we can't name an absolute URL for), in which
/// case the message falls back to carrying the raw id.
fn health_tab_url(ws: Uuid) -> Option<String> {
    let base = oxy_app_core::custom_apps_host_dispatch::admin_base_url()?;
    Some(format!("{base}/admin/workspaces/{ws}?tab=health"))
}

/// Render a workspace reference for Slack as a link straight to its Health tab,
/// so an operator clicks through instead of copying a UUID into the admin search.
/// The bare id only appears when we can't build an absolute URL.
fn workspace_ref(ws: Uuid, label: Option<&WorkspaceLabel>) -> String {
    let url = health_tab_url(ws);
    match (label, url) {
        (Some(l), Some(url)) => {
            let name = escape(&l.name);
            match &l.org_name {
                Some(org) => format!("*<{url}|{name}>* ({})", escape(org)),
                None => format!("*<{url}|{name}>*"),
            }
        }
        (Some(l), None) => {
            let name = escape(&l.name);
            match &l.org_name {
                Some(org) => format!("*{name}* ({}) `{ws}`", escape(org)),
                None => format!("*{name}* `{ws}`"),
            }
        }
        (None, Some(url)) => format!("<{url}|{ws}>"),
        (None, None) => format!("`{ws}`"),
    }
}

/// Coarse "how long has this been broken" for reminder headers: `45m`, `3h 20m`,
/// `2d 4h`. Deliberately imprecise — the exact instant is on the Health tab.
fn humanize(d: chrono::Duration) -> String {
    let mins = d.num_minutes().max(0);
    match (mins / 1440, (mins % 1440) / 60, mins % 60) {
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (days, h, _) => format!("{days}d {h}h"),
    }
}

/// One health notification: who it's about, what's wrong, and why we're sending
/// it. Grouped so [`push_slack`] stays a four-argument call.
pub struct HealthAlert<'a> {
    pub workspace_id: Uuid,
    pub label: Option<&'a WorkspaceLabel>,
    pub status: HealthStatus,
    pub reasons: &'a [String],
    pub decision: AlertDecision,
    /// When the workspace entered its current status (`changed_at`), used for the
    /// "for 3h 20m" suffix on reminders. `None` on a first-ever eval.
    pub since: Option<DateTime<FixedOffset>>,
    pub now: DateTime<Utc>,
}

impl HealthAlert<'_> {
    /// How long the current status has held, when we know.
    fn held_for(&self) -> Option<String> {
        let since = self.since?;
        Some(humanize(
            self.now.signed_duration_since(since.with_timezone(&Utc)),
        ))
    }

    fn header(&self) -> Option<String> {
        let ws_ref = workspace_ref(self.workspace_id, self.label);
        let held = self
            .held_for()
            .map(|d| format!(" for {d}"))
            .unwrap_or_default();
        match self.decision {
            AlertDecision::Alert => Some(format!(
                ":rotating_light: Workspace {ws_ref} is {}",
                self.status.as_str()
            )),
            AlertDecision::Reminder { escalated: true } => Some(format!(
                ":rotating_light: Workspace {ws_ref} is still unhealthy{held} — new failures"
            )),
            AlertDecision::Reminder { escalated: false } => Some(format!(
                ":warning: Workspace {ws_ref} is still unhealthy{held}"
            )),
            AlertDecision::Recovery => Some(format!(
                ":white_check_mark: Workspace {ws_ref} recovered (healthy)"
            )),
            AlertDecision::Silent => None,
        }
    }
}

/// Post a health alert / reminder / recovery message to the ops Slack channel.
pub async fn push_slack(
    client: &SlackClient,
    bot_token: &str,
    channel: &str,
    alert: &HealthAlert<'_>,
) -> Result<(), OxyError> {
    let Some(header) = alert.header() else {
        return Ok(());
    };
    let body = if alert.reasons.is_empty() {
        header
    } else {
        format!("{header}\n• {}", alert.reasons.join("\n• "))
    };
    client
        .chat_post_message(bot_token, channel, &body, None)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluator::HealthDimension;
    use crate::evaluator::HealthStatus::*;

    fn at(hours_ago: i64) -> DateTime<FixedOffset> {
        (Utc::now() - chrono::Duration::hours(hours_ago)).fixed_offset()
    }

    fn fail(dimension: HealthDimension, status: HealthStatus) -> DimensionFailure {
        DimensionFailure { dimension, status }
    }

    /// Transition-only baseline: no prior alert, no failures, reminders on.
    fn input(prev: Option<HealthStatus>, next: HealthStatus) -> AlertInput<'static> {
        AlertInput {
            prev,
            next,
            alerted_failures: None,
            next_failures: &[],
            last_alerted_at: None,
            now: Utc::now(),
            reminder_after: Some(chrono::Duration::hours(6)),
        }
    }

    #[test]
    fn first_time_unhealthy_alerts() {
        assert_eq!(
            decide_transition(&input(None, Unhealthy)),
            AlertDecision::Alert
        );
    }

    #[test]
    fn healthy_to_unhealthy_alerts() {
        assert_eq!(
            decide_transition(&input(Some(Healthy), Unhealthy)),
            AlertDecision::Alert
        );
    }

    #[test]
    fn degraded_to_unhealthy_alerts() {
        assert_eq!(
            decide_transition(&input(Some(Degraded), Unhealthy)),
            AlertDecision::Alert
        );
    }

    #[test]
    fn unhealthy_within_the_interval_is_silent() {
        let failures = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let i = AlertInput {
            alerted_failures: Some(&failures),
            next_failures: &failures,
            last_alerted_at: Some(at(1)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(decide_transition(&i), AlertDecision::Silent);
    }

    #[test]
    fn unhealthy_past_the_interval_reminds() {
        // The bug this fixes: a workspace stuck unhealthy used to go quiet forever
        // after the first page.
        let failures = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let i = AlertInput {
            alerted_failures: Some(&failures),
            next_failures: &failures,
            last_alerted_at: Some(at(7)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(
            decide_transition(&i),
            AlertDecision::Reminder { escalated: false }
        );
    }

    #[test]
    fn unhealthy_never_alerted_reminds_immediately() {
        // A row written before alert tracking existed, or a workspace whose every
        // push failed. Silence there is the failure mode we're fixing.
        let i = AlertInput {
            last_alerted_at: None,
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(
            decide_transition(&i),
            AlertDecision::Reminder { escalated: false }
        );
    }

    #[test]
    fn a_new_failing_dimension_repages_before_the_interval() {
        let alerted = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let now = vec![
            fail(HealthDimension::JobLiveness, Unhealthy),
            fail(HealthDimension::SmokeTest, Unhealthy),
        ];
        let i = AlertInput {
            alerted_failures: Some(&alerted),
            next_failures: &now,
            last_alerted_at: Some(at(1)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(
            decide_transition(&i),
            AlertDecision::Reminder { escalated: true }
        );
    }

    #[test]
    fn a_dimension_getting_worse_repages_before_the_interval() {
        // Degraded → unhealthy on a dimension we already paged about is a real
        // escalation, even though the set of failing dimensions is unchanged.
        let alerted = vec![fail(HealthDimension::Reconciliation, Degraded)];
        let now = vec![fail(HealthDimension::Reconciliation, Unhealthy)];
        let i = AlertInput {
            alerted_failures: Some(&alerted),
            next_failures: &now,
            last_alerted_at: Some(at(1)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(
            decide_transition(&i),
            AlertDecision::Reminder { escalated: true }
        );
    }

    #[test]
    fn churning_counts_on_the_same_dimension_do_not_repage() {
        // The alert-storm regression: reason text embeds live counts
        // ("4/12 runs failed in window (33%)"), which move on nearly every ~10m
        // pass for a workspace that is continuously broken on the same dimension.
        // Diffing text paged every pass and bypassed the interval entirely;
        // diffing dimensions holds the reminder cadence.
        let alerted = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let still_the_same_failure = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let i = AlertInput {
            alerted_failures: Some(&alerted),
            next_failures: &still_the_same_failure,
            last_alerted_at: Some(at(1)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(decide_transition(&i), AlertDecision::Silent);
    }

    #[test]
    fn a_dimension_recovering_does_not_repage() {
        // Fewer failures, or a milder one, is good news — it shortens the next
        // reminder rather than earning an out-of-band page.
        let alerted = vec![
            fail(HealthDimension::JobLiveness, Unhealthy),
            fail(HealthDimension::Queue, Unhealthy),
        ];
        let fewer = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let milder = vec![
            fail(HealthDimension::JobLiveness, Unhealthy),
            fail(HealthDimension::Queue, Degraded),
        ];
        for next in [&fewer, &milder] {
            let i = AlertInput {
                alerted_failures: Some(&alerted),
                next_failures: next,
                last_alerted_at: Some(at(1)),
                ..input(Some(Unhealthy), Unhealthy)
            };
            assert_eq!(decide_transition(&i), AlertDecision::Silent);
        }
    }

    #[test]
    fn reordered_failures_are_not_an_escalation() {
        let alerted = vec![
            fail(HealthDimension::JobLiveness, Unhealthy),
            fail(HealthDimension::Queue, Unhealthy),
        ];
        let reordered = vec![
            fail(HealthDimension::Queue, Unhealthy),
            fail(HealthDimension::JobLiveness, Unhealthy),
        ];
        let i = AlertInput {
            alerted_failures: Some(&alerted),
            next_failures: &reordered,
            last_alerted_at: Some(at(1)),
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(decide_transition(&i), AlertDecision::Silent);
    }

    #[test]
    fn reminders_can_be_disabled() {
        let failures = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let i = AlertInput {
            alerted_failures: Some(&failures),
            next_failures: &failures,
            last_alerted_at: Some(at(500)),
            reminder_after: None,
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(decide_transition(&i), AlertDecision::Silent);
    }

    #[test]
    fn disabled_reminders_also_silence_an_escalation() {
        // `OXY_HEALTH_REALERT_HOURS=0` is documented as transition-only alerting.
        // A new failing dimension on an already-unhealthy workspace is still a
        // repeat, so it stays silent — the transition into unhealthy already paged.
        let alerted = vec![fail(HealthDimension::JobLiveness, Unhealthy)];
        let now = vec![
            fail(HealthDimension::JobLiveness, Unhealthy),
            fail(HealthDimension::SmokeTest, Unhealthy),
        ];
        let i = AlertInput {
            alerted_failures: Some(&alerted),
            next_failures: &now,
            last_alerted_at: Some(at(1)),
            reminder_after: None,
            ..input(Some(Unhealthy), Unhealthy)
        };
        assert_eq!(decide_transition(&i), AlertDecision::Silent);
    }

    #[test]
    fn unhealthy_to_healthy_recovers() {
        assert_eq!(
            decide_transition(&input(Some(Unhealthy), Healthy)),
            AlertDecision::Recovery
        );
    }

    #[test]
    fn healthy_to_degraded_is_silent() {
        // Degraded is dashboard-only; we alert Slack on unhealthy transitions.
        assert_eq!(
            decide_transition(&input(Some(Healthy), Degraded)),
            AlertDecision::Silent
        );
    }

    #[test]
    fn first_time_healthy_is_silent() {
        assert_eq!(
            decide_transition(&input(None, Healthy)),
            AlertDecision::Silent
        );
    }

    #[test]
    fn reminder_interval_defaults_and_disables() {
        // SAFETY: nextest gives each test its own process — no concurrent env access.
        unsafe { std::env::remove_var("OXY_HEALTH_REALERT_HOURS") };
        assert_eq!(reminder_interval(), Some(chrono::Duration::hours(6)));
        unsafe { std::env::set_var("OXY_HEALTH_REALERT_HOURS", "1") };
        assert_eq!(reminder_interval(), Some(chrono::Duration::hours(1)));
        unsafe { std::env::set_var("OXY_HEALTH_REALERT_HOURS", "0") };
        assert_eq!(reminder_interval(), None);
        unsafe { std::env::set_var("OXY_HEALTH_REALERT_HOURS", "nonsense") };
        assert_eq!(reminder_interval(), Some(chrono::Duration::hours(6)));
    }

    fn label(org: Option<&str>) -> WorkspaceLabel {
        WorkspaceLabel {
            name: "Acme Analytics".into(),
            org_name: org.map(str::to_string),
        }
    }

    #[test]
    fn workspace_ref_links_to_the_health_tab() {
        // SAFETY: nextest gives each test its own process.
        unsafe { std::env::set_var("OXY_API_URL", "https://app.oxygen-hq.com/api") };
        let ws = Uuid::nil();
        let url = format!("https://app.oxygen-hq.com/admin/workspaces/{ws}?tab=health");
        assert_eq!(
            workspace_ref(ws, Some(&label(Some("Acme Corp")))),
            format!("*<{url}|Acme Analytics>* (Acme Corp)")
        );
        assert_eq!(
            workspace_ref(ws, Some(&label(None))),
            format!("*<{url}|Acme Analytics>*")
        );
        // No label resolved (workspace deleted between sweep and alert): the id is
        // still clickable.
        assert_eq!(workspace_ref(ws, None), format!("<{url}|{ws}>"));
    }

    #[test]
    fn workspace_ref_falls_back_to_the_id_without_a_base_url() {
        // SAFETY: nextest gives each test its own process.
        unsafe { std::env::remove_var("OXY_API_URL") };
        let ws = Uuid::nil();
        assert_eq!(
            workspace_ref(ws, Some(&label(Some("Acme Corp")))),
            format!("*Acme Analytics* (Acme Corp) `{ws}`")
        );
        assert_eq!(
            workspace_ref(ws, Some(&label(None))),
            format!("*Acme Analytics* `{ws}`")
        );
        assert_eq!(workspace_ref(ws, None), format!("`{ws}`"));
    }

    #[test]
    fn tenant_names_cannot_break_out_of_the_link() {
        // SAFETY: nextest gives each test its own process.
        unsafe { std::env::set_var("OXY_API_URL", "https://app.oxygen-hq.com/api") };
        let ws = Uuid::nil();
        let l = WorkspaceLabel {
            name: "<https://evil.example|click me>".into(),
            org_name: Some("A & B".into()),
        };
        let rendered = workspace_ref(ws, Some(&l));
        assert!(rendered.contains("&lt;https://evil.example|click me&gt;"));
        assert!(rendered.contains("(A &amp; B)"));
    }

    #[test]
    fn humanize_reads_as_an_outage_duration() {
        assert_eq!(humanize(chrono::Duration::minutes(45)), "45m");
        assert_eq!(humanize(chrono::Duration::minutes(200)), "3h 20m");
        assert_eq!(humanize(chrono::Duration::hours(52)), "2d 4h");
        // A clock skew that puts `changed_at` in the future must not underflow.
        assert_eq!(humanize(chrono::Duration::minutes(-5)), "0m");
    }

    #[test]
    fn reminder_header_names_the_duration_and_the_cause() {
        // SAFETY: nextest gives each test its own process.
        unsafe { std::env::set_var("OXY_API_URL", "https://app.oxygen-hq.com/api") };
        let now = Utc::now();
        let alert = HealthAlert {
            workspace_id: Uuid::nil(),
            label: None,
            status: Unhealthy,
            reasons: &[],
            decision: AlertDecision::Reminder { escalated: true },
            since: Some((now - chrono::Duration::hours(3)).fixed_offset()),
            now,
        };
        let header = alert.header().unwrap();
        assert!(header.contains("still unhealthy for 3h 0m — new failures"));
        assert!(header.contains("?tab=health"));
    }

    #[test]
    fn silent_renders_nothing() {
        let alert = HealthAlert {
            workspace_id: Uuid::nil(),
            label: None,
            status: Healthy,
            reasons: &[],
            decision: AlertDecision::Silent,
            since: None,
            now: Utc::now(),
        };
        assert!(alert.header().is_none());
    }
}
