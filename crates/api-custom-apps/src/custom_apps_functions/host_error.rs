//! What a failed `ctx.*` call or invocation is, before it is a sentence.
//!
//! Every host method used to return `Result<_, String>`, and the *start* of
//! that string was the contract twice over: `failure_signal` and
//! `host_call_attrs` classify a failure by reading it, and `@oxy-hq/sdk`'s
//! test context repeats the words (`sdk_testing_drift`,
//! `cli_capabilities_drift`). A sentence reworded for any reason silently
//! reclassified a failure.
//!
//! [`HostError`] carries the kind beside the text, so the platform's own code
//! can decide on the kind and a reworded sentence can no longer move a
//! failure to another kind. **The text is still the contract**: [`Display`]
//! is `message`, byte for byte, and `message` is what the isolate throws
//! (`runtime::ops::reply_json`), what the audit rows store and what the SDK
//! copies. Nothing here renames a sentence.
//!
//! A kind is *derived from*, never *replaces*, the text, so the two routes
//! meet in one place: [`HostError::from`] reads a string's head the way the
//! classifiers always did. A string that still arrives untyped (an older
//! path, a row read back from the database) gets the kind its prefix names,
//! and a constructor's kind equals the one its own text parses to — a test
//! per kind holds that.
//!
//! [`Display`]: std::fmt::Display

use super::env_policy::{HELD_LABEL, REFUSED_LABEL};

/// What a function sees when the isolate's own code threw, ahead of the
/// message (`RuntimeError::Js`).
pub(super) const THREW_HEAD: &str = "function threw:";
/// The JS `name` of the error a failed `ctx.*` call throws into the isolate
/// (`runtime::ops::reply_json`). Behind [`THREW_HEAD`] it says the handler let
/// a platform call's failure escape rather than raising one of its own.
pub(super) const HOST_ERROR_NAME: &str = "HostError";
/// What a runtime failure that is the platform's, not the app's, says first
/// (`RuntimeError::Internal`); the classifier has always matched it without
/// the colon.
const INTERNAL_HEAD: &str = "internal runtime error";
/// The whole text of a heap-ceiling breach (`RuntimeError::OutOfMemory`).
const MEMORY_LIMIT_TEXT: &str = "function exceeded its memory limit";
/// The tail of every `<Area>CapabilityMissing` label.
const CAPABILITY_MISSING_SUFFIX: &str = "CapabilityMissing";
/// How `ctx.secrets.set`'s refusal begins: the one gate that carries no label.
const SECRETS_GATE: &str = "ctx.secrets.set: this function has not declared the";

/// The `<Area>` of an `<Area>CapabilityMissing` refusal: the manifest gate the
/// function lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityArea {
    /// `secrets.write` — the one gate whose words carry no `<Area>` label.
    Secrets,
    Email,
    Notify,
    Org,
    Platform,
    Storage,
    Oltp,
    Airhouse,
}

impl CapabilityArea {
    const ALL: [Self; 8] = [
        Self::Secrets,
        Self::Email,
        Self::Notify,
        Self::Org,
        Self::Platform,
        Self::Storage,
        Self::Oltp,
        Self::Airhouse,
    ];

    /// The word before `CapabilityMissing`; `None` for the unlabelled gate.
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Secrets => None,
            Self::Email => Some("Email"),
            Self::Notify => Some("Notify"),
            Self::Org => Some("Org"),
            Self::Platform => Some("Platform"),
            Self::Storage => Some("Storage"),
            Self::Oltp => Some("Oltp"),
            Self::Airhouse => Some("Airhouse"),
        }
    }

    fn of_label(stem: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.label() == Some(stem))
    }
}

/// How a call or an invocation failed, in the terms the platform decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostErrorKind {
    /// The manifest lacks a capability the op needs; refused by the broker
    /// before the host method runs.
    CapabilityMissing { area: CapabilityArea },
    /// The environment held the op by not running it (`HeldInStaging`).
    Held,
    /// The environment refused the op (`EnvironmentRefused`).
    Refused,
    /// Everything else a host method reports: a bad argument, a warehouse or
    /// storage failure, an unknown op. Classified by its text
    /// (`host_call_attrs::classify_host_error_for`), which is why the text
    /// stays the contract.
    Backend,
    /// The app's own code threw (`function threw: …`).
    Threw,
    /// The runtime failed, not the app (`internal runtime error: …`).
    Internal,
    /// The isolate breached its heap ceiling.
    MemoryLimit,
}

/// A failed host call or invocation: its kind, and the exact text the isolate,
/// the audit rows and the SDK contract see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError {
    pub kind: HostErrorKind,
    /// The whole text, prefix included. [`Display`](std::fmt::Display) prints
    /// it unchanged.
    pub message: String,
}

impl HostError {
    /// A failure with no more specific kind. Reads like any untyped string
    /// would not: it is `Backend` whatever its text, for the host code that
    /// composes its own words.
    pub fn backend(message: impl Into<String>) -> Self {
        Self {
            kind: HostErrorKind::Backend,
            message: message.into(),
        }
    }

    /// A refusal for a capability the manifest lacks. `message` is the host's
    /// own sentence, label included — written where `sdk_testing_drift`
    /// scans for it.
    pub fn capability_missing(area: CapabilityArea, message: impl Into<String>) -> Self {
        Self {
            kind: HostErrorKind::CapabilityMissing { area },
            message: message.into(),
        }
    }

    /// An op the environment held; `message` starts `HeldInStaging: `.
    pub fn held(message: impl Into<String>) -> Self {
        Self {
            kind: HostErrorKind::Held,
            message: message.into(),
        }
    }

    /// An op the environment refused; `message` starts `EnvironmentRefused: `.
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            kind: HostErrorKind::Refused,
            message: message.into(),
        }
    }

    /// The app's code threw `inner`.
    pub fn threw(inner: &str) -> Self {
        Self {
            kind: HostErrorKind::Threw,
            message: format!("{THREW_HEAD} {inner}"),
        }
    }

    /// The runtime failed with `inner`.
    pub fn internal(inner: &str) -> Self {
        Self {
            kind: HostErrorKind::Internal,
            message: format!("{INTERNAL_HEAD}: {inner}"),
        }
    }

    /// The isolate breached its heap ceiling.
    pub fn memory_limit() -> Self {
        Self {
            kind: HostErrorKind::MemoryLimit,
            message: MEMORY_LIMIT_TEXT.to_string(),
        }
    }

    /// The text, as the isolate throws it.
    pub fn as_str(&self) -> &str {
        &self.message
    }
}

/// The kind a string's head names — the prefix parsing every classifier did
/// before kinds existed, now in one place and reached only for a string that
/// arrives without one.
fn kind_of_text(message: &str) -> HostErrorKind {
    if message.starts_with(THREW_HEAD) {
        return HostErrorKind::Threw;
    }
    if message.starts_with(INTERNAL_HEAD) {
        return HostErrorKind::Internal;
    }
    if message.starts_with(MEMORY_LIMIT_TEXT) {
        return HostErrorKind::MemoryLimit;
    }
    if message.starts_with(SECRETS_GATE) {
        return HostErrorKind::CapabilityMissing {
            area: CapabilityArea::Secrets,
        };
    }
    match message.split_once(": ").map(|(head, _)| head) {
        Some(HELD_LABEL) => HostErrorKind::Held,
        Some(REFUSED_LABEL) => HostErrorKind::Refused,
        Some(head) => head
            .strip_suffix(CAPABILITY_MISSING_SUFFIX)
            .and_then(CapabilityArea::of_label)
            .map_or(HostErrorKind::Backend, |area| {
                HostErrorKind::CapabilityMissing { area }
            }),
        None => HostErrorKind::Backend,
    }
}

/// The transition: a string from a path not yet typed, kind read off its head.
impl From<String> for HostError {
    fn from(message: String) -> Self {
        Self {
            kind: kind_of_text(&message),
            message,
        }
    }
}

impl From<&str> for HostError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HostError {}

/// The text as the `str` it always was, so a caller (and a test) that asks
/// whether it `contains` or `starts_with` a phrase reads it unchanged.
impl std::ops::Deref for HostError {
    type Target = str;

    fn deref(&self) -> &str {
        &self.message
    }
}

impl PartialEq<String> for HostError {
    fn eq(&self, other: &String) -> bool {
        &self.message == other
    }
}

/// Lets a test write `assert_eq!(err, "text")` against the contract text.
impl PartialEq<str> for HostError {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for HostError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

#[cfg(test)]
#[path = "host_error_tests.rs"]
mod tests;
