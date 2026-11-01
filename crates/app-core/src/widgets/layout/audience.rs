//! Who a widget is for: the `for` of a widgets group or an item.
//!
//! `{ "people": ["scott@poke.house"], "teams": ["Office"] }` — only those people
//! (by email) or members of those org teams (by name) draw the copy. Absent: everyone
//! who can open the app. It only ever narrows: the app's own access applies first,
//! and a group's `for` and an item's `for` must both admit the viewer.
//!
//! Emails and team names match case-insensitively; both are stored lowercased and
//! written back that way. An address or a team nobody has simply matches nobody:
//! that is never an error when the home is drawn. `internal-docs/home-widgets.md`.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{Problem, problem};

/// The most entries one of `people` / `teams` may list.
pub const MAX_ENTRIES: usize = 50;

/// The people and teams a group or a copy is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audience {
    /// Lowercased emails.
    pub people: Vec<String>,
    /// Lowercased org team names.
    pub teams: Vec<String>,
}

/// Who is looking: their email and the names of the org teams they belong to in
/// the org being drawn. A plain value, so the layout model needs no database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewer {
    email: Option<String>,
    teams: BTreeSet<String>,
}

impl Viewer {
    /// `email` is `None` for a frontline worker enrolled without a mailbox: they
    /// can still be reached through a team.
    pub fn new(email: Option<&str>, teams: impl IntoIterator<Item = String>) -> Self {
        Self {
            email: email
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty()),
            teams: teams.into_iter().map(|t| t.trim().to_lowercase()).collect(),
        }
    }
}

impl Audience {
    /// Whether `viewer` is listed, or in a listed team.
    pub fn admits(&self, viewer: &Viewer) -> bool {
        viewer
            .email
            .as_ref()
            .is_some_and(|e| self.people.contains(e))
            || self.teams.iter().any(|t| viewer.teams.contains(t))
    }

    /// The `for` object a layout document carries.
    pub fn to_value(&self) -> Value {
        let mut doc = Map::new();
        if !self.people.is_empty() {
            doc.insert("people".into(), json!(self.people));
        }
        if !self.teams.is_empty() {
            doc.insert("teams".into(), json!(self.teams));
        }
        Value::Object(doc)
    }

    /// The strict read of a `for` found at `at` (its own path, e.g.
    /// `sections[0].for`): `None` when absent, or when it has problems, which are
    /// then in `problems`.
    pub(super) fn parse(v: Option<&Value>, at: &str, problems: &mut Vec<Problem>) -> Option<Self> {
        let v = v?;
        let Some(obj) = v.as_object() else {
            problems.push(problem(
                at,
                "`for` is an object with `people` and/or `teams`",
                "Write { \"people\": [\"scott@poke.house\"], \"teams\": [\"Office\"] }, or leave `for` out for everyone.",
            ));
            return None;
        };
        let before = problems.len();
        for key in obj
            .keys()
            .filter(|k| !["people", "teams"].contains(&k.as_str()))
        {
            problems.push(problem(
                format!("{at}.{key}"),
                format!("`{key}` is not a field of `for`"),
                "Remove it; `for` takes people and teams.",
            ));
        }
        let people = list(
            obj,
            "people",
            at,
            problems,
            |s| s.contains('@'),
            |at| {
                problem(
                    at,
                    "a person is an email address",
                    "Write the address, e.g. \"scott@poke.house\".",
                )
            },
        );
        let teams = list(
            obj,
            "teams",
            at,
            problems,
            |s| !s.is_empty(),
            |at| {
                problem(
                    at,
                    "a team is the name of an org team",
                    "Write the team's name, e.g. \"Office\"; it cannot be empty.",
                )
            },
        );
        if people.is_empty() && teams.is_empty() && problems.len() == before {
            problems.push(problem(
                at,
                "`for` names nobody",
                "List at least one person or team, or leave `for` out for everyone.",
            ));
        }
        (problems.len() == before).then_some(Self { people, teams })
    }
}

/// One of `for`'s lists, trimmed, lowercased and deduped. `valid` says whether an
/// entry is usable; `bad` builds the problem for one that is not.
fn list(
    obj: &Map<String, Value>,
    key: &str,
    at: &str,
    problems: &mut Vec<Problem>,
    valid: impl Fn(&str) -> bool,
    bad: impl Fn(String) -> Problem,
) -> Vec<String> {
    let path = format!("{at}.{key}");
    let Some(raw) = obj.get(key) else {
        return Vec::new();
    };
    let Some(entries) = raw.as_array() else {
        problems.push(problem(
            path,
            format!("`{key}` is a list of strings"),
            format!("Write \"{key}\": [ … ]."),
        ));
        return Vec::new();
    };
    if entries.len() > MAX_ENTRIES {
        problems.push(problem(
            path.clone(),
            format!(
                "`{key}` lists {} entries; at most {MAX_ENTRIES}",
                entries.len()
            ),
            "Split the widgets between groups, or use a team.",
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let text = entry.as_str().map(|s| s.trim().to_lowercase());
        match text.filter(|s| valid(s)) {
            Some(s) if !out.contains(&s) => out.push(s),
            Some(_) => {}
            None => problems.push(bad(format!("{path}[{i}]"))),
        }
    }
    out
}
