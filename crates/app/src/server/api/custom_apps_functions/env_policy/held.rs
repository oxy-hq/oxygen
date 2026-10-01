//! What a held or refused host op says, and what a held op resolves to.
//!
//! Kept beside the policy rather than in the host sources: the labels here
//! (`HeldInStaging`, `EnvironmentRefused`) are not the app's argument errors
//! and not the platform failing, so `host_call_attrs::classify_host_error`
//! reads them as their own kinds (`held`, `refused`), which never page.

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::HostOp;

/// The label at the head of every held op's error.
pub const HELD_LABEL: &str = "HeldInStaging";

/// The label at the head of every refused op's error.
pub const REFUSED_LABEL: &str = "EnvironmentRefused";

/// The error a held op throws. Names the op, why it did not happen, and where
/// the call is recorded — the shape of `oxyc proxy`'s 409.
pub fn held_message(op: HostOp, environment: &AppEnvironment) -> String {
    format!(
        "{HELD_LABEL}: ctx.{op} was not performed. This invocation runs in the \
         {environment} environment, where writes are held until the store has an isolated \
         copy; in production it would have run. It is listed in this invocation's \
         app.staging.held audit row.",
        op = op.name(),
    )
}

/// The error a refused op throws (design §4.2 `EnvironmentRefused`).
pub fn refused_message(op: HostOp, environment: &AppEnvironment, fix: &str) -> String {
    format!(
        "{REFUSED_LABEL}: ctx.{op} is refused in the {environment} environment: {fix}.",
        op = op.name(),
    )
}

/// The error a refused op throws in a production-admitted run that reads a
/// branch (previews S9, I9): `reason` says how the run was found to read one.
pub fn branch_refused_message(op: HostOp, reason: &str, fix: &str) -> String {
    format!(
        "{REFUSED_LABEL}: ctx.{op} isn't available in a staging or preview invocation: \
         {reason}. {fix}.",
        op = op.name(),
    )
}

/// The error a held op throws in a production-admitted run that reads a
/// branch — a preview, held like staging (previews S9, I9).
pub fn branch_held_message(op: HostOp, reason: &str) -> String {
    format!(
        "{HELD_LABEL}: ctx.{op} was not performed: this invocation reads a branch ({reason}), \
         so it runs as a preview and its writes are held.",
        op = op.name(),
    )
}

/// The error a held OLTP statement throws: what it was and why it was not
/// sent. `why` names the statement's verb, or the function it calls.
/// `held` is the op's held message ([`held_message`] or
/// [`branch_held_message`]).
pub fn held_statement_message(held: &str, why: &str) -> String {
    format!("{held} Only a single read statement is sent outside production; this one {why}.")
}

/// The closed read set for `ctx.fetch`: a read method is sent in every
/// environment, anything else is a write.
pub fn is_read_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    )
}

/// What a held mutating `ctx.fetch` resolves to: a 409, as `oxyc proxy`
/// answers, so code that checks `res.status` sees a failure without the flow
/// throwing. Names the host only — a URL's path or query can carry a secret.
/// `held` is the op's held message.
pub fn held_fetch_response(method: &str, host: Option<&str>, held: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "error": "held_in_staging",
        "message": format!("{held} (a {method} to {})", host.unwrap_or("<host>")),
    });
    serde_json::json!({
        "status": 409,
        "body": body.to_string(),
        "encoding": "utf8",
        "held": true,
    })
}

/// What a held `ctx.email.send` resolves to: no mail, and the recipients and
/// subject production would have used, back to the function that chose them.
/// `held` is the op's held message.
pub fn held_email_result(input: &serde_json::Value, held: &str) -> serde_json::Value {
    serde_json::json!({
        "held": true,
        "reason": held,
        "to": input.get("to").cloned().unwrap_or(serde_json::Value::Null),
        "subject": input.get("subject").cloned().unwrap_or(serde_json::Value::Null),
    })
}

/// Postgres's answer to a write inside a `READ ONLY` transaction (SQLSTATE
/// 25006), recognised by its message: connectors surface errors as strings.
pub fn is_read_only_violation(err: &str) -> bool {
    err.contains("read-only transaction") || err.contains("25006")
}
