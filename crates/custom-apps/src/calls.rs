//! App-to-app calls (`ctx.apps.call` and `ctx.apps.send`), the manifest's side
//! and the constants both sides of a call agree on.
//!
//! The callee marks a function `"callable": true`; the caller lists the pairs
//! it may reach under a top-level `"calls": { "<app-slug>": ["<function>"] }`.
//! Both are read from what a build declares, never from the call.
//!
//! A call runs the callee's own build, in the caller's environment. With a
//! person behind the chain it runs as that person, after the platform asks
//! whether they may open the target app (`Action::AppCall`); a background chain
//! and a delivered send run as the system. See
//! `internal-docs/customer-apps-functions.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_json::Value;

/// The most (app, function) pairs a manifest's `calls` may name.
pub const MAX_CALL_PAIRS: usize = 64;
/// The largest JSON body a call carries each way, in bytes. A call crosses the
/// isolate boundary as one string, so it is held far below a route's transport
/// cap.
pub const MAX_CALL_BODY_BYTES: usize = 1024 * 1024;
/// How many characters of a callee's thrown message reach the caller.
pub const MAX_CALL_ERROR_CHARS: usize = 500;
/// Time kept back from the caller's budget for it to handle the answer: a
/// callee never gets the whole of what remains.
pub const BUDGET_MARGIN: Duration = Duration::from_millis(250);
/// The prefix of the outbox name a `ctx.apps.send` is written under.
pub const SEND_NAME_PREFIX: &str = "call:";

/// Why a call did not return the callee's answer. The label heads the text the
/// caller's isolate sees (`<Label>: <text>`), which the SDK turns into
/// `AppCallError.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallErrorKind {
    /// The call was not allowed: undeclared, bad arguments, events off.
    Refused,
    /// The target app or function does not exist in this org.
    NotFound,
    /// The function exists but is not marked `callable`.
    NotCallable,
    /// The target app has no build in this environment.
    NotDeployed,
    /// The callee did not answer within its budget.
    Timeout,
    /// The callee ran and failed.
    Failed,
    /// No isolate came free within the budget.
    Busy,
    /// The chain is too deep.
    DepthExceeded,
    /// The target (app, function) is already on this synchronous chain.
    Cycle,
}

impl CallErrorKind {
    pub const ALL: [CallErrorKind; 9] = [
        Self::Refused,
        Self::NotFound,
        Self::NotCallable,
        Self::NotDeployed,
        Self::Timeout,
        Self::Failed,
        Self::Busy,
        Self::DepthExceeded,
        Self::Cycle,
    ];

    /// The word heading the host's message.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Refused => "AppCallRefused",
            Self::NotFound => "AppCallNotFound",
            Self::NotCallable => "AppCallNotCallable",
            Self::NotDeployed => "AppCallNotDeployed",
            Self::Timeout => "AppCallTimeout",
            Self::Failed => "AppCallFailed",
            Self::Busy => "AppCallBusy",
            Self::DepthExceeded => "AppCallDepthExceeded",
            Self::Cycle => "AppCallCycle",
        }
    }

    /// The host's message for this kind: `<Label>: <text>`.
    ///
    /// Each label is written whole beside its text, so the host's typed-label
    /// scan (`host_call_attrs`) finds them in this file.
    pub fn message(self, text: impl AsRef<str>) -> String {
        let text = text.as_ref();
        match self {
            Self::Refused => format!("AppCallRefused: {text}"),
            Self::NotFound => format!("AppCallNotFound: {text}"),
            Self::NotCallable => format!("AppCallNotCallable: {text}"),
            Self::NotDeployed => format!("AppCallNotDeployed: {text}"),
            Self::Timeout => format!("AppCallTimeout: {text}"),
            Self::Failed => format!("AppCallFailed: {text}"),
            Self::Busy => format!("AppCallBusy: {text}"),
            Self::DepthExceeded => format!("AppCallDepthExceeded: {text}"),
            Self::Cycle => format!("AppCallCycle: {text}"),
        }
    }

    /// The kind a message's label names, if it has one.
    pub fn of_message(message: &str) -> Option<Self> {
        let (head, _) = message.split_once(':')?;
        Self::ALL.into_iter().find(|k| k.label() == head)
    }
}

/// The refusal for a call to `target` (app, function) when it is already on
/// `chain`, the (app, function) pairs of the synchronous chain so far, the
/// caller last. `None` when the target is not on it: a different function of
/// an app on the chain is not a cycle.
pub fn cycle_refusal(chain: &[(String, String)], target: (&str, &str)) -> Option<String> {
    chain
        .iter()
        .any(|(app, function)| (app.as_str(), function.as_str()) == target)
        .then(|| {
            let steps: Vec<String> = chain
                .iter()
                .map(|(app, function)| format!("{app}/{function}"))
                .chain(std::iter::once(format!("{}/{}", target.0, target.1)))
                .collect();
            CallErrorKind::Cycle.message(format!(
                "this call would run `{}/{}` again inside itself: {}",
                target.0,
                target.1,
                steps.join(" -> ")
            ))
        })
}

/// `text` cut to [`MAX_CALL_ERROR_CHARS`] characters, marked when cut.
pub fn truncate_error(text: &str) -> String {
    if text.chars().count() <= MAX_CALL_ERROR_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_CALL_ERROR_CHARS).collect();
    format!("{cut}…")
}

/// The pairs a manifest's top-level `calls` declares, by app slug.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredCalls {
    pub apps: BTreeMap<String, BTreeSet<String>>,
}

fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn function_name_ok(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && s.len() <= 64
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl DeclaredCalls {
    /// Read the manifest's `calls` (`None` or absent: none declared). An error
    /// names what is wrong, for publish to refuse the build with.
    pub fn from_manifest(manifest: Option<&Value>) -> Result<Self, String> {
        let Some(block) = manifest.and_then(|m| m.get("calls")) else {
            return Ok(Self::default());
        };
        let block = block
            .as_object()
            .ok_or("`calls` must be an object of app slug to a list of function names")?;
        let mut apps = BTreeMap::new();
        let mut pairs = 0usize;
        for (app, functions) in block {
            if !slug_ok(app) {
                return Err(format!("calls: `{app}` is not an app slug"));
            }
            let list = functions
                .as_array()
                .filter(|l| !l.is_empty())
                .ok_or_else(|| {
                    format!("calls.{app}: must be a non-empty list of function names")
                })?;
            let mut names = BTreeSet::new();
            for item in list {
                let name = item
                    .as_str()
                    .filter(|n| function_name_ok(n))
                    .ok_or_else(|| format!("calls.{app}: {item} is not a function name"))?;
                if !names.insert(name.to_string()) {
                    return Err(format!("calls.{app}: `{name}` is listed twice"));
                }
            }
            pairs += names.len();
            apps.insert(app.clone(), names);
        }
        if pairs > MAX_CALL_PAIRS {
            return Err(format!(
                "calls names {pairs} functions; at most {MAX_CALL_PAIRS} are allowed"
            ));
        }
        Ok(Self { apps })
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    pub fn allows(&self, app: &str, function: &str) -> bool {
        self.apps.get(app).is_some_and(|f| f.contains(function))
    }

    /// Every declared pair, in order.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.apps
            .iter()
            .flat_map(|(a, fs)| fs.iter().map(move |f| (a.as_str(), f.as_str())))
    }
}

/// Whether a function's manifest entry says `"callable": true`.
pub fn function_callable(entry: Option<&Value>) -> bool {
    entry
        .and_then(|e| e.get("callable"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The budget a callee gets: the smallest of what the caller asked (`asked`),
/// the callee function's own timeout and what remains of the caller's budget
/// less [`BUDGET_MARGIN`]. `None` when nothing is left, which is a timeout
/// without starting the callee.
pub fn callee_budget(
    asked: Option<Duration>,
    callee_timeout: Duration,
    caller_remaining: Duration,
) -> Option<Duration> {
    let left = caller_remaining.checked_sub(BUDGET_MARGIN)?;
    if left.is_zero() {
        return None;
    }
    let budget = left.min(callee_timeout).min(asked.unwrap_or(Duration::MAX));
    (!budget.is_zero()).then_some(budget)
}

/// The outbox name a send to `app`'s `function` from `caller_function` is
/// written under: `call:<app>:<function>:<caller function>`. No part holds a
/// `:` (slugs and function names are lowercase letters, digits and hyphens).
pub fn send_name(app: &str, function: &str, caller_function: &str) -> String {
    format!("{SEND_NAME_PREFIX}{app}:{function}:{caller_function}")
}

/// The target and caller a [`send_name`] names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    pub app: String,
    pub function: String,
    pub caller_function: String,
}

impl SendTarget {
    /// `None` when `name` is not a send's.
    pub fn parse(name: &str) -> Option<Self> {
        let rest = name.strip_prefix(SEND_NAME_PREFIX)?;
        let mut parts = rest.split(':');
        let (app, function, caller_function) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || !slug_ok(app) || !function_name_ok(function) {
            return None;
        }
        if !function_name_ok(caller_function) {
            return None;
        }
        Some(Self {
            app: app.into(),
            function: function.into(),
            caller_function: caller_function.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declared_calls_read_and_refuse() {
        let ok = json!({ "calls": { "bookkeeping": ["get-mappings", "ping"] } });
        let got = DeclaredCalls::from_manifest(Some(&ok)).unwrap();
        assert!(got.allows("bookkeeping", "ping"));
        assert!(!got.allows("bookkeeping", "other"));
        assert!(!got.allows("nope", "ping"));
        for bad in [
            json!({ "calls": [] }),
            json!({ "calls": { "Bad": ["x"] } }),
            json!({ "calls": { "a": [] } }),
            json!({ "calls": { "a": ["x", "x"] } }),
            json!({ "calls": { "a": ["X"] } }),
        ] {
            assert!(DeclaredCalls::from_manifest(Some(&bad)).is_err(), "{bad}");
        }
        assert!(DeclaredCalls::from_manifest(None).unwrap().is_empty());
    }

    #[test]
    fn pair_cap_is_enforced() {
        let fns: Vec<String> = (0..65).map(|i| format!("f{i}")).collect();
        let m = json!({ "calls": { "a": fns } });
        assert!(
            DeclaredCalls::from_manifest(Some(&m))
                .unwrap_err()
                .contains("at most")
        );
    }

    #[test]
    fn budget_is_the_smallest_of_three() {
        let s = Duration::from_secs;
        assert_eq!(callee_budget(Some(s(5)), s(10), s(30)), Some(s(5)));
        assert_eq!(callee_budget(None, s(10), s(30)), Some(s(10)));
        assert_eq!(
            callee_budget(Some(s(60)), s(100), s(3)),
            Some(s(3) - BUDGET_MARGIN)
        );
        assert_eq!(callee_budget(None, s(10), BUDGET_MARGIN), None);
        assert_eq!(callee_budget(None, s(10), Duration::ZERO), None);
    }

    #[test]
    fn labels_round_trip_and_send_names_parse() {
        for kind in CallErrorKind::ALL {
            assert_eq!(CallErrorKind::of_message(&kind.message("x")), Some(kind));
            assert_eq!(kind.message("x"), format!("{}: x", kind.label()));
        }
        assert_eq!(CallErrorKind::of_message("host trouble: x"), None);
        let name = send_name("bookkeeping", "get-mappings", "poster");
        let t = SendTarget::parse(&name).unwrap();
        assert_eq!(
            (t.app.as_str(), t.function.as_str()),
            ("bookkeeping", "get-mappings")
        );
        assert_eq!(t.caller_function, "poster");
        assert!(SendTarget::parse("statement.uploaded").is_none());
        assert!(SendTarget::parse("call:a:b").is_none());
        assert!(SendTarget::parse("call:a:b:c:d").is_none());
        assert_eq!(
            truncate_error(&"x".repeat(600)).chars().count(),
            MAX_CALL_ERROR_CHARS + 1
        );
    }

    #[test]
    fn callable_reads_the_function_entry() {
        assert!(function_callable(Some(&json!({ "callable": true }))));
        assert!(!function_callable(Some(&json!({ "callable": "yes" }))));
        assert!(!function_callable(None));
    }
}
