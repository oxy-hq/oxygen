//! What a failed invocation says on the platform's side.
//!
//! In 0.5.140–0.5.144 every `ctx.warehouse.insert` against ClickHouse failed,
//! and every failure was recorded — in `app_function_invocations.error` and the
//! tenant-facing ClickHouse sink. Neither is a place anyone on call looks. No
//! platform log line named the failure, the invocation span was never marked
//! failed, and nothing paged; a customer reported it, and a first-pass
//! investigation that searched the platform logs found nothing and guessed.
//!
//! This is the shape of a failure the platform can hold without holding its
//! message: a coarse [`Failure::kind`] (a HyperDX facet, a Slack line) and a
//! [`fingerprint`] that names "the same failure" across invocations and apps.
//! The message itself stays in the tenant's store, behind the app-admin gate —
//! the same rule `host_call_attrs` keeps for host-op spans. A fingerprint is
//! pseudonymous rather than anonymous: it hides the message, but someone who
//! already holds a message and a guessed value can check the guess against it.

use sha2::{Digest, Sha256};

/// How an invocation failed, with nothing of what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Failure {
    /// `threw` (the app's code threw) | `internal` (the runtime failed) |
    /// `platform` (the invocation never reached the app's code) | `timeout` |
    /// `http_5xx` | `host_call` (the app answered below 500 after a `ctx.*`
    /// call failed). Bounded on purpose: a facet with one value per message is
    /// no facet.
    pub kind: &'static str,
    /// See [`fingerprint`].
    pub fingerprint: String,
    /// The call behind a `host_call` failure, by op and kind; `None` for every
    /// other kind. Carried because that row's `error` is NULL — the app caught
    /// the message — so these two closed-list values are what the page and the
    /// log line can name.
    pub host_call: Option<HostCallFailure>,
}

/// The first `ctx.*` call of an invocation that failed on the platform's side,
/// by name only. `op` is a fixed op name (`query`, `warehouse.insert`), never
/// the SQL, URL or secret name the call carried; `kind` is a
/// `host_call_attrs::classify_host_error` value that counts toward paging.
// Only the V8 runtime constructs one; without it the type is still named by
// the run outcome, which always carries `None`.
//
// `pub`, not `pub(super)`: `FunctionHost::host_call_failure` is a method of a
// `pub` trait, and a `pub(super)` return type there trips `private_interfaces`.
// This module is private, so the type still reaches no further than
// `custom_apps_functions`.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCallFailure {
    pub op: &'static str,
    pub kind: &'static str,
}

/// The rule for the note a host keeps across one invocation's calls. The host
/// holds the state (`ProjectFunctionHost::host_call_failure`); the broker
/// reports each call's outcome; these decide what the note becomes.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
impl HostCallFailure {
    /// After a call of `op` failed as `kind`: the first failure stands. One
    /// page names one failure, and later calls often fail because of it.
    pub fn noted(current: Option<Self>, op: &'static str, kind: &'static str) -> Option<Self> {
        current.or(Some(Self { op, kind }))
    }

    /// After a call of `op` succeeded: a failure of that op is one the run
    /// recovered from (a retried `ctx.fetch` that answered on the second
    /// attempt), and the fingerprint should name a failure it did not. By op
    /// name only — two targets under one op share the clear, because the op
    /// name comes from a closed list and a target would not.
    pub fn recovered(current: Option<Self>, op: &'static str) -> Option<Self> {
        current.filter(|hc| hc.op != op)
    }
}

impl Failure {
    /// The failure an invocation's outcome describes, or `None` when it did not
    /// fail. A cancellation is someone asking it to stop, not the app failing.
    /// A handler that caught an error and answered 5xx did fail — that is how a
    /// broken host call looks from an app that handles its errors. So did one
    /// that answered below 500 after a host call failed (`host_call`): catching
    /// the error hides it from the response, not from on-call.
    pub fn of(
        status: &str,
        http_status: u16,
        error: Option<&str>,
        host_call: Option<&HostCallFailure>,
    ) -> Option<Self> {
        let (kind, fingerprint) = match status {
            "cancelled" => return None,
            "success" if http_status < 500 => {
                return host_call.map(|hc| Self {
                    kind: "host_call",
                    fingerprint: digest(&format!("host_call {} {}", hc.op, hc.kind)),
                    host_call: Some(*hc),
                });
            }
            // Digested whole, not normalized: the status IS the signature, and
            // normalizing would fold a new 500 into a function's usual 503.
            "success" => ("http_5xx", digest(&format!("http status {http_status}"))),
            "timeout" => ("timeout", Self::timeout_fingerprint()),
            _ => {
                let message = error.unwrap_or_default();
                (kind_of_error(message), fingerprint(message))
            }
        };
        Some(Self {
            kind,
            fingerprint,
            host_call: None,
        })
    }

    /// The fingerprint every timeout shares, for the reaper, which marks
    /// orphaned rows `timeout` without running [`Failure::of`] per row.
    pub fn timeout_fingerprint() -> String {
        digest("timeout")
    }

    /// Mark the enclosing invocation span failed and put one WARN line on the
    /// platform log. Call inside the `custom_app_function` span: the line then
    /// carries that span's app, function, invocation and request ids, which is
    /// how on-call gets from HyperDX to the invocation row and its message.
    #[cfg(feature = "custom-app-functions")]
    pub fn report(&self) {
        let span = tracing::Span::current();
        span.record("otel.status_code", "ERROR");
        span.record("error.type", self.kind);
        // A `host_call` failure's message went to the app's catch block, not
        // the invocation row, so the op and kind are what on-call has to go on.
        let (op, host_kind) = self
            .host_call
            .map_or((None, None), |hc| (Some(hc.op), Some(hc.kind)));
        span.record("host_call.op", op);
        span.record("host_call.kind", host_kind);
        // Braced: `type` is a keyword, so the field is a string literal, and
        // unbraced the macro cannot tell a literal field from the message.
        tracing::warn!(
            target: "oxy::app_function",
            {
                error.fingerprint = %self.fingerprint,
                "error.type" = %self.kind,
                host_call.op = op,
                host_call.kind = host_kind
            },
            "custom-app function invocation failed"
        );
    }
}

/// Who an `error` status belongs to, from the prefix `RuntimeError` puts on the
/// message. Anything without one failed before the isolate ran — a workspace
/// or build lookup, a missing function — which is the platform's, not the app's.
fn kind_of_error(message: &str) -> &'static str {
    if message.starts_with("function threw:") {
        "threw"
    } else if message.starts_with("internal runtime error") {
        "internal"
    } else {
        "platform"
    }
}

/// Bytes of normalized message that feed the digest. Past this, messages that
/// share a prefix are the same failure; stack traces and quoted rows vary
/// further down without saying anything new.
const FINGERPRINT_PREFIX: usize = 240;

/// A stable name for "the same failure": 16 hex chars of a digest over the
/// message with its data taken out.
///
/// Two invocations that failed the same way differ in everything the message
/// says about the data — quoted values, row numbers, ids, the rows ClickHouse
/// quotes back. Quoted runs become `?` and every word containing a digit
/// becomes `#` before hashing, so the Code 27 insert failure is one
/// fingerprint across every invocation and every app it hit.
pub(super) fn fingerprint(message: &str) -> String {
    let normalized = normalize(message);
    let end = normalized
        .char_indices()
        .nth(FINGERPRINT_PREFIX)
        .map_or(normalized.len(), |(i, _)| i);
    digest(&normalized[..end])
}

fn digest(input: &str) -> String {
    hex::encode(&Sha256::digest(input.as_bytes())[..8])
}

fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(FINGERPRINT_PREFIX * 2));
    let mut chars = message.chars().peekable();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.chars().any(|c| c.is_ascii_digit()) {
            out.push('#');
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            // A quote opens a quoted run only where a value could start: after
            // a word it is an apostrophe (`doesn't`), and treating it as a quote
            // would swallow the rest of the message.
            '\'' | '"' | '`' if word.is_empty() => {
                flush(&mut word, &mut out);
                skip_quoted(c, &mut chars);
                out.push('?');
            }
            c if c.is_alphanumeric() || c == '_' => word.push(c),
            c if c.is_whitespace() => {
                flush(&mut word, &mut out);
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => {
                flush(&mut word, &mut out);
                out.push(c);
            }
        }
    }
    flush(&mut word, &mut out);
    out.trim().to_string()
}

/// Consume a quoted run up to its closing `quote`, honouring `\`-escapes and a
/// doubled quote. An unterminated run swallows the rest, which is still one `?`.
fn skip_quoted(quote: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
            } else {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error every ClickHouse insert returned in 0.5.140–0.5.144, as two
    /// different invocations of two different apps saw it.
    const CODE_27_A: &str = "function threw: Error: warehouse insert failed: query failed: HTTP 400 \
        Bad Request: Code: 27. DB::Exception: Cannot parse input: expected '(' before: \
        '/*oxy.app=\\'bookkeeping\\',oxy.fn=\\'upload-report\\',oxy.invocation=\\'0af7651916cd\\'*/': \
        at row 63: While executing ValuesBlockInputFormat. (CANNOT_PARSE_INPUT_ASSERTION_FAILED) \
        (version 25.8.4.13 (official build))";
    const CODE_27_B: &str = "function threw: Error: warehouse insert failed: query failed: HTTP 400 \
        Bad Request: Code: 27. DB::Exception: Cannot parse input: expected '(' before: \
        '/*oxy.app=\\'receiving\\',oxy.fn=\\'submit\\',oxy.invocation=\\'884e14953bc3\\'*/': \
        at row 2: While executing ValuesBlockInputFormat. (CANNOT_PARSE_INPUT_ASSERTION_FAILED) \
        (version 25.8.4.13 (official build))";

    #[test]
    fn the_same_failure_in_different_apps_and_rows_is_one_fingerprint() {
        assert_eq!(fingerprint(CODE_27_A), fingerprint(CODE_27_B));
        assert_eq!(fingerprint(CODE_27_A).len(), 16);
    }

    #[test]
    fn a_different_failure_is_a_different_fingerprint() {
        let missing_table = "function threw: Error: warehouse insert failed: query failed: HTTP 404 \
            Not Found: Code: 60. DB::Exception: Table default.receiving does not exist. \
            (UNKNOWN_TABLE) (version 25.8.4.13 (official build))";
        assert_ne!(fingerprint(CODE_27_A), fingerprint(missing_table));
        assert_ne!(
            fingerprint("function threw: Error: name is required"),
            fingerprint("function threw: Error: partySize is required")
        );
    }

    #[test]
    fn the_data_in_a_message_does_not_reach_the_fingerprint_input() {
        let normalized = normalize(CODE_27_A);
        for leaked in ["bookkeeping", "upload-report", "0af7651916cd", "63"] {
            assert!(!normalized.contains(leaked), "{leaked} in {normalized}");
        }
        assert_eq!(
            normalize("row 'it''s' at \"col\" id 4bf92f35 uuid 0b9f4c7e-1d2a"),
            "row ? at ? id # uuid #-#"
        );
        // An apostrophe is not a quote: the words after it still count.
        assert_ne!(
            fingerprint("Table t doesn't exist. (UNKNOWN_TABLE)"),
            fingerprint("Table t doesn't exist. (UNKNOWN_DATABASE)")
        );
    }

    #[test]
    fn only_a_failed_outcome_is_a_failure() {
        assert_eq!(Failure::of("success", 200, None, None), None);
        assert_eq!(
            Failure::of("success", 404, None, None),
            None,
            "a 4xx is an answer"
        );
        assert_eq!(
            Failure::of("cancelled", 0, Some("function was cancelled"), None),
            None
        );

        let caught = Failure::of("success", 500, None, None).expect("a 5xx is a failure");
        assert_eq!(caught.kind, "http_5xx");
        assert_ne!(
            caught.fingerprint,
            Failure::of("success", 503, None, None).unwrap().fingerprint
        );

        assert_eq!(
            Failure::of("timeout", 0, Some("function execution timed out"), None)
                .unwrap()
                .kind,
            "timeout"
        );
        assert_eq!(
            Failure::of(
                "error",
                0,
                Some("internal runtime error: isolate died"),
                None
            )
            .unwrap()
            .kind,
            "internal"
        );
        let threw = Failure::of("error", 0, Some(CODE_27_A), None).unwrap();
        assert_eq!(threw.kind, "threw");
        assert_eq!(threw.fingerprint, fingerprint(CODE_27_B));
        // Never reached the app's code: the platform's failure, not the app's.
        assert_eq!(
            Failure::of(
                "error",
                0,
                Some("workspace lookup failed: connection refused"),
                None
            )
            .unwrap()
            .kind,
            "platform"
        );
        assert_eq!(
            Failure::of("timeout", 0, None, None).unwrap().fingerprint,
            Failure::timeout_fingerprint(),
            "the reaper's timeouts group with the runtime's"
        );
    }

    #[test]
    fn a_caught_host_call_failure_on_a_2xx_is_a_failure() {
        let hc = HostCallFailure {
            op: "warehouse.insert",
            kind: "host_call_failed",
        };
        let f = Failure::of("success", 200, None, Some(&hc)).expect("caught host failure pages");
        assert_eq!(f.kind, "host_call");
        assert_eq!(
            f.fingerprint,
            digest("host_call warehouse.insert host_call_failed")
        );
        assert_eq!(f.host_call, Some(hc), "the page names the op and kind");
    }

    /// The first failure stands over a later one, and a later success of the
    /// same op clears it: an app that retried and got its answer pages nobody.
    /// A success of another op clears nothing — a `storage.head` that worked
    /// says nothing about the `fetch` that did not.
    #[test]
    fn a_noted_host_call_failure_is_cleared_by_a_success_of_the_same_op() {
        let noted = HostCallFailure::noted(None, "fetch", "timeout");
        assert_eq!(
            noted,
            Some(HostCallFailure {
                op: "fetch",
                kind: "timeout"
            })
        );
        assert_eq!(
            HostCallFailure::noted(noted, "warehouse.insert", "host_call_failed"),
            noted,
            "the first failure stands"
        );
        assert_eq!(HostCallFailure::recovered(noted, "storage.head"), noted);
        assert_eq!(HostCallFailure::recovered(noted, "fetch"), None);
        // Recovered, then failed again: the second failure is the one the run
        // did not recover from.
        assert_eq!(
            HostCallFailure::noted(
                HostCallFailure::recovered(noted, "fetch"),
                "warehouse.insert",
                "host_call_failed",
            ),
            Some(HostCallFailure {
                op: "warehouse.insert",
                kind: "host_call_failed"
            })
        );
    }

    #[test]
    fn a_host_call_failure_does_not_reclassify_a_real_failure() {
        let hc = HostCallFailure {
            op: "query",
            kind: "timeout",
        };
        let real = Failure::of("success", 503, None, Some(&hc)).unwrap();
        assert_eq!(real.kind, "http_5xx");
        assert_eq!(real.host_call, None, "the page names the 5xx, not the call");
        assert_eq!(
            Failure::of("timeout", 0, None, Some(&hc)).unwrap().kind,
            "timeout"
        );
        assert_eq!(Failure::of("cancelled", 0, None, Some(&hc)), None);
    }
}
