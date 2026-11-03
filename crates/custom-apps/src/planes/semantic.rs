//! What the `semantic` plane's row says: pin the semantic revision the build
//! was published with, or read a git branch of the workspace's semantic model
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §3 row 2).
//!
//! The mode that follows from the choice is [`SemanticMode`]
//! (`build-pin` or `branch`); the branch name lives only here, in the
//! `PlaneSpec`; `ResolvedPlanes` carries the mode alone.
//!
//! `release:staging` (read the semantic revision a data release last served in
//! staging) is deliberately not a choice yet: it needs W4's data releases, and
//! adding the spelling before then would parse a value nothing can honor.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer};
use serde::{Serialize, Serializer};

use super::SemanticMode;

/// The longest branch name a spec accepts, in bytes (git itself allows far
/// more; the column is a small JSON object and a name this long is a mistake).
const MAX_BRANCH_BYTES: usize = 200;

/// A `semantic` value in a `planes` column: the string `"build-pin"` or
/// `"branch:<git branch>"`, or the object form `{"mode": "branch", "branch":
/// "<git branch>"}` (read, never written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticChoice {
    /// The revision the build was published with (the default).
    BuildPin,
    /// A git branch of the workspace's semantic model.
    Branch(String),
}

impl SemanticChoice {
    /// The mode this choice resolves to.
    pub const fn mode(&self) -> SemanticMode {
        match self {
            SemanticChoice::BuildPin => SemanticMode::BuildPin,
            SemanticChoice::Branch(_) => SemanticMode::Branch,
        }
    }

    /// Parse the wire string. Errors name the `semantic` plane.
    fn parse(value: &str) -> Result<SemanticChoice, String> {
        if value == "build-pin" {
            return Ok(SemanticChoice::BuildPin);
        }
        if let Some(name) = value.strip_prefix("branch:") {
            return valid_branch_name(name)
                .map(|()| SemanticChoice::Branch(name.to_string()))
                .map_err(|why| format!("semantic: branch name {name:?} is not usable: {why}"));
        }
        if value == "branch" {
            return Err(
                "semantic: mode \"branch\" needs a branch name, written \"branch:<git branch>\""
                    .to_string(),
            );
        }
        Err(format!(
            "semantic: unknown mode {value:?}, expected \"build-pin\" or \"branch:<git branch>\""
        ))
    }
}

impl SemanticChoice {
    /// Parse the object form `{"mode": …, "branch": …}`.
    fn from_object(object: &serde_json::Map<String, serde_json::Value>) -> Result<Self, String> {
        if let Some(key) = object.keys().find(|k| *k != "mode" && *k != "branch") {
            return Err(format!("semantic: unknown setting {key:?}"));
        }
        let text = |key: &str| match object.get(key) {
            None => Ok(None),
            Some(serde_json::Value::String(value)) => Ok(Some(value.as_str())),
            Some(other) => Err(format!("semantic: {key} must be a string, got {other}")),
        };
        match (text("mode")?, text("branch")?) {
            (Some("build-pin"), None) => Ok(SemanticChoice::BuildPin),
            (Some("build-pin"), Some(_)) => {
                Err("semantic: mode \"build-pin\" takes no branch".to_string())
            }
            (Some("branch"), Some(name)) => SemanticChoice::parse(&format!("branch:{name}")),
            (Some("branch"), None) => {
                Err("semantic: mode \"branch\" needs a branch name, written \
                 {\"mode\": \"branch\", \"branch\": \"<git branch>\"}"
                    .to_string())
            }
            (Some(other), _) => Err(format!(
                "semantic: unknown mode {other:?}, expected \"build-pin\" or \"branch\""
            )),
            (None, _) => Err("semantic: the object form needs a \"mode\"".to_string()),
        }
    }
}

impl fmt::Display for SemanticChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticChoice::BuildPin => f.write_str("build-pin"),
            SemanticChoice::Branch(name) => write!(f, "branch:{name}"),
        }
    }
}

impl Serialize for SemanticChoice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SemanticChoice {
    /// The wire string (`"build-pin"`, `"branch:<b>"`), or the object form a
    /// plane with settings is written in: `{"mode": "branch", "branch": "<b>"}`
    /// (`{"mode": "build-pin"}` for the default). Both give the same choice;
    /// serializing always writes the string.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::String(value) => {
                SemanticChoice::parse(&value).map_err(de::Error::custom)
            }
            serde_json::Value::Object(object) => {
                SemanticChoice::from_object(&object).map_err(de::Error::custom)
            }
            other => Err(de::Error::custom(format!(
                "semantic: expected \"build-pin\", \"branch:<git branch>\" or \
                 {{\"mode\": \"branch\", \"branch\": \"<git branch>\"}}, got {other}"
            ))),
        }
    }
}

/// Whether `name` can be a branch a spec names: git's own `check-ref-format`
/// rules for a branch name, so a name that passes here is safe to hand to a ref
/// lookup. Rejects an empty name or one over 200 bytes; control characters,
/// whitespace and any of `~ ^ : ? * [ \`; `..` and `@{`; a doubled `/`; a
/// leading `/` or `-`; a trailing `/` or `.`; a path component that starts with
/// `.` or ends with `.lock`; and the bare name `@`.
pub fn valid_branch_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("it is empty".to_string());
    }
    if name.len() > MAX_BRANCH_BYTES {
        return Err(format!("it is longer than {MAX_BRANCH_BYTES} bytes"));
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_control() || c.is_whitespace() || FORBIDDEN_CHARS.contains(*c))
    {
        return Err(format!("it contains {c:?}"));
    }
    if name == "@" {
        return Err("it is the bare name \"@\"".to_string());
    }
    for (needle, why) in [("..", "\"..\""), ("@{", "\"@{\""), ("//", "\"//\"")] {
        if name.contains(needle) {
            return Err(format!("it contains {why}"));
        }
    }
    if name.starts_with('-') || name.starts_with('/') {
        return Err("it starts with \"-\" or \"/\"".to_string());
    }
    if name.ends_with('/') || name.ends_with('.') {
        return Err("it ends with \"/\" or \".\"".to_string());
    }
    if let Some(part) = name
        .split('/')
        .find(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(format!(
            "its component {part:?} starts with \".\" or ends with \".lock\""
        ));
    }
    Ok(())
}

/// Characters git refuses in a ref name, beyond control characters and
/// whitespace (checked separately).
const FORBIDDEN_CHARS: &str = "~^:?*[\\";
