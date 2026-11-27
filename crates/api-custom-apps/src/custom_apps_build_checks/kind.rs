//! What a check is (its kind), where it stands (its status), and what it found
//! (its outcome and findings): the vocabulary every other module and the wire
//! share.

use serde::Serialize;
use serde_json::{Value, json};

/// One kind of check an attempt is queued with. A staging attempt has phase
/// 1's four, and `smoke` and `changes` too where the promote checks gate (the
/// org's `promote_checks_gate` flag) is on; a production attempt has
/// `production_smoke` alone. The kinds a model writes ([`MODEL_KINDS`]) are
/// rows every reader renders by label and findings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckKind {
    /// The bundle's recorded validation (`app_builds.validation_status`).
    Validation,
    /// Every function the build marks `"check": true`, run on staging.
    Functions,
    /// Headless Chromium opens staging at desk and phone width.
    Loads,
    /// The SDK floor and the theme guard on the bundle's CSS.
    DesignSystem,
    /// Every screen of the build opens on staging and throws nothing.
    Smoke,
    /// What differs from production, screen by screen. Advisory: it informs,
    /// and holds a promote only through "a person has seen it".
    Changes,
    /// After a production move: every screen opens on production and shows no
    /// error it did not show before the move. A production attempt's one kind.
    /// Never a gate: a failure alerts, and rolls back where the app says so.
    ProductionSmoke,
}

/// The kinds of rows a model writes, never a runner or the platform's own
/// checks: the design audit, and the build's summary note. They are added to
/// an attempt after it has settled, so they never keep one unsettled.
pub const MODEL_KINDS: [&str; 2] = ["design_audit", "summary"];

/// The words the console and `oxyc` show for the `design_audit` row.
pub const DESIGN_AUDIT_LABEL: &str = "Design audit";

/// Whether a stored kind is a model's row ([`MODEL_KINDS`]).
pub fn is_models(raw: &str) -> bool {
    MODEL_KINDS.contains(&raw)
}

impl CheckKind {
    /// What a staging move queues where the gate is off, in the order a
    /// build's list shows them.
    pub const PHASE_ONE: [CheckKind; 4] = [
        CheckKind::Validation,
        CheckKind::Functions,
        CheckKind::Loads,
        CheckKind::DesignSystem,
    ];

    /// What the gate adds to a staging attempt, after phase 1's, in the
    /// order a build's list shows them.
    pub const GATED: [CheckKind; 2] = [CheckKind::Smoke, CheckKind::Changes];

    /// The kinds a staging attempt is queued with: phase 1's, and the gate's
    /// two where the org's `promote_checks_gate` flag is on.
    pub fn of_staging_attempt(gated: bool) -> Vec<CheckKind> {
        let mut kinds = Self::PHASE_ONE.to_vec();
        if gated {
            kinds.extend(Self::GATED);
        }
        kinds
    }

    /// What a production attempt is queued with: the one check that looks at
    /// production after a move.
    pub const PRODUCTION: [CheckKind; 1] = [CheckKind::ProductionSmoke];

    /// The stored `kind`: a wire contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Functions => "functions",
            Self::Loads => "loads",
            Self::DesignSystem => "design_system",
            Self::Smoke => "smoke",
            Self::Changes => "changes",
            Self::ProductionSmoke => "production_smoke",
        }
    }

    /// The words the console and `oxyc` show.
    pub fn label(self) -> &'static str {
        match self {
            Self::Validation => "Builds and validates",
            Self::Functions => "Its checks pass",
            Self::Loads => "It loads, desk and phone",
            Self::DesignSystem => "Follows the design system",
            Self::Smoke => "Every screen opens",
            Self::Changes => "What changed",
            Self::ProductionSmoke => "Production opens after promote",
        }
    }

    /// Whether the kind only informs: its row is written `advisory`, so it is
    /// never part of an attempt's verdict and can never be required.
    pub fn advisory(self) -> bool {
        matches!(self, Self::Changes)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::of_staging_attempt(true)
            .into_iter()
            .chain(Self::PRODUCTION)
            .find(|k| k.as_str() == raw)
    }

    /// The label of a stored kind: its own, the design audit's, or the kind's
    /// words for one this build does not know (a later phase's).
    pub fn label_of(raw: &str) -> String {
        if raw == MODEL_KINDS[0] {
            return DESIGN_AUDIT_LABEL.to_string();
        }
        Self::parse(raw).map_or_else(|| raw.replace('_', " "), |k| k.label().to_string())
    }

    /// Where a stored kind sits in a build's list: phase 1's in their order,
    /// anything later after them.
    pub fn order_of(raw: &str) -> usize {
        Self::PHASE_ONE
            .iter()
            .position(|k| k.as_str() == raw)
            .unwrap_or(Self::PHASE_ONE.len())
    }

    /// The key a build's list is sorted by: phase 1's kinds in their order
    /// ([`Self::order_of`]), then the gate's two, then anything later by name.
    pub fn list_key(raw: &str) -> (usize, usize, &str) {
        let gated = Self::GATED
            .iter()
            .position(|k| k.as_str() == raw)
            .unwrap_or(Self::GATED.len());
        (Self::order_of(raw), gated, raw)
    }
}

/// Where one check stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Queued,
    Running,
    Passed,
    Failed,
    /// The check could not reach a verdict: what it needed was missing or
    /// broken (a store read, a misconfigured checker, an unknown SDK version).
    Error,
    /// Nothing to check, or nothing to check it with. A skip nobody decided
    /// by judging the build is marked so ([`Outcome::not_checked`]).
    Skipped,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        [
            Self::Queued,
            Self::Running,
            Self::Passed,
            Self::Failed,
            Self::Error,
            Self::Skipped,
        ]
        .into_iter()
        .find(|s| s.as_str() == raw)
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// How bad one finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One thing a check found, as every kind's `detail.findings` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
    /// Where: a `file:line`, a URL, or nothing.
    pub at: Option<String>,
    /// The rule that found it, when one did (`restates-sdk-token`).
    pub rule: Option<String>,
}

/// The longest finding message stored. Findings carry a page error's text or a
/// function's error, which an app writes; the cap keeps a row a verdict, not
/// a copy of what the app printed.
pub const MAX_MESSAGE_CHARS: usize = 400;
/// The most findings one check stores; the summary counts the rest.
pub const MAX_FINDINGS: usize = 50;

impl Finding {
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: cap(&message.into(), MAX_MESSAGE_CHARS),
            at: None,
            rule: None,
        }
    }

    pub fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(cap(&at.into(), MAX_MESSAGE_CHARS));
        self
    }

    pub fn rule(mut self, rule: impl Into<String>) -> Self {
        self.rule = Some(rule.into());
        self
    }
}

/// `text` cut to `max` characters, marked when cut.
pub fn cap(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// What one check concluded: the row's status, summary and detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: CheckStatus,
    pub summary: String,
    /// `{findings, ...}`: always an object with a `findings` array.
    pub detail: Value,
}

impl Outcome {
    /// An outcome whose detail is `extra` (an object) with `findings` set.
    pub fn new(status: CheckStatus, summary: impl Into<String>, findings: &[Finding]) -> Self {
        Self::with(status, summary, findings, json!({}))
    }

    pub fn with(
        status: CheckStatus,
        summary: impl Into<String>,
        findings: &[Finding],
        extra: Value,
    ) -> Self {
        let mut detail = match extra {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        let kept: Vec<&Finding> = findings.iter().take(MAX_FINDINGS).collect();
        detail["findings"] = serde_json::to_value(kept).unwrap_or_else(|_| json!([]));
        if findings.len() > MAX_FINDINGS {
            detail["findings_omitted"] = json!(findings.len() - MAX_FINDINGS);
        }
        Self {
            status,
            summary: cap(&summary.into(), 200),
            detail,
        }
    }

    /// A check that could not reach a verdict, and why.
    pub fn error(summary: impl Into<String>) -> Self {
        let summary = summary.into();
        let finding = Finding::new(Severity::Error, summary.clone());
        Self::new(CheckStatus::Error, summary, &[finding])
    }

    /// A check with nothing to check, and why: the check's own decision (no
    /// check functions, no recorded SDK version), so it counts beside a pass.
    pub fn skipped(summary: impl Into<String>) -> Self {
        Self::new(CheckStatus::Skipped, summary, &[])
    }

    /// A check nobody judged, and why: the platform closed it because what it
    /// would have looked at was gone (its environment no longer served the
    /// build). `skipped` on the wire, with [`NOT_CHECKED_KEY`] set in its
    /// detail, so no reader takes it for a pass (`dto::not_checked`).
    pub fn not_checked(summary: impl Into<String>) -> Self {
        let marked = json!({ NOT_CHECKED_KEY: true });
        Self::with(CheckStatus::Skipped, summary, &[], marked)
    }
}

/// The key the platform sets (`true`) in the detail of a check it closed
/// without judging it ([`Outcome::not_checked`]). A report never sets it
/// (`runner::report::PLATFORM_KEYS`).
pub const NOT_CHECKED_KEY: &str = "not_checked";

#[cfg(test)]
#[path = "kind_tests.rs"]
mod tests;
