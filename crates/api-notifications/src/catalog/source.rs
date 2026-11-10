//! One source's catalogue — Oxygen's, or one app's live build — and what it
//! says about a kind.
//!
//! A kind the catalogue does not describe still has an answer: the implicit
//! `general` category, a label made from its name, and a level from the
//! functions that send it. So a build with no `notifications` block, a kind
//! received from an older build, and a platform kind from a newer binary all
//! resolve the same way.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{
    Action, Catalogue, GENERAL, GENERAL_LABEL, Level, ancestors, humanize, is_within,
};
use crate::delivery::Channel;
use crate::platform::kinds as platform_kinds;

/// Whose notifications a catalogue describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Oxygen,
    App,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Oxygen => "oxygen",
            Source::App => "app",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceCatalogue {
    pub source: Source,
    pub catalogue: Catalogue,
    /// The kinds a function may send (`functions.*.notify.kinds`), each with
    /// whether one marks it critical. Empty for Oxygen.
    pub declared: BTreeMap<String, bool>,
}

/// Everything the tree, the inbox and delivery need to know about one kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindInfo {
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    pub level: Level,
    /// Its category path; [`GENERAL`] when it names none.
    pub category: String,
    /// The labels from the top category down to its own.
    pub category_labels: Vec<String>,
    pub actions: Vec<Action>,
    /// The channels it can be delivered on, besides the inbox.
    pub channels: Vec<&'static str>,
    /// Whether a person's mutes apply to it: not to a critical kind, nor to
    /// an Oxygen kind that is never muted (`kinds::is_mutable`: a test
    /// notice, which exists to show that push arrives).
    pub mutable: bool,
}

impl KindInfo {
    pub fn critical(&self) -> bool {
        self.level == Level::Critical
    }

    /// Whether settings draw it: a switch that can do something, or a
    /// critical kind shown locked on. A test notice has neither.
    pub fn has_switch(&self) -> bool {
        self.mutable || self.critical()
    }
}

impl SourceCatalogue {
    pub fn app(catalogue: Catalogue, declared: BTreeMap<String, bool>) -> Self {
        Self {
            source: Source::App,
            catalogue,
            declared,
        }
    }

    /// The category `kind` sits in.
    pub fn category_of(&self, kind: &str) -> String {
        self.catalogue
            .kinds
            .get(kind)
            .and_then(|k| k.category.clone())
            .unwrap_or_else(|| GENERAL.to_string())
    }

    /// A function's `critical: true` makes a kind critical whatever the
    /// catalogue says: the display never under-reports what ignores a mute.
    pub fn level_of(&self, kind: &str) -> Level {
        if self.declared.get(kind) == Some(&true) {
            return Level::Critical;
        }
        self.catalogue
            .kinds
            .get(kind)
            .and_then(|k| k.level)
            .unwrap_or_default()
    }

    /// Oxygen's kinds have fixed channels; an app's reach all three, since
    /// each call picks email and Slack itself.
    pub fn channels_of(&self, kind: &str) -> Vec<&'static str> {
        match self.source {
            Source::Oxygen => platform_kinds::channels(kind).to_vec(),
            Source::App => Channel::ALL.iter().map(|c| c.as_str()).collect(),
        }
    }

    pub fn describe(&self, kind: &str) -> KindInfo {
        let def = self.catalogue.kinds.get(kind);
        let category = self.category_of(kind);
        let level = self.level_of(kind);
        let mutable = level != Level::Critical
            && (self.source == Source::App || platform_kinds::is_mutable(kind));
        KindInfo {
            kind: kind.to_string(),
            label: def
                .and_then(|d| d.label.clone())
                .unwrap_or_else(|| humanize(kind)),
            description: def.and_then(|d| d.description.clone()),
            level,
            category_labels: self.labels(&category),
            category,
            actions: def.map(|d| d.actions.clone()).unwrap_or_default(),
            channels: self.channels_of(kind),
            mutable,
        }
    }

    pub fn label_of(&self, path: &str) -> String {
        match self
            .catalogue
            .categories
            .get(path)
            .and_then(|c| c.label.clone())
        {
            Some(label) => label,
            None if path == GENERAL => GENERAL_LABEL.to_string(),
            None => humanize(path.rsplit('/').next().unwrap_or(path)),
        }
    }

    pub fn description_of(&self, path: &str) -> Option<String> {
        self.catalogue
            .categories
            .get(path)
            .and_then(|c| c.description.clone())
    }

    /// Labels from the top of `path` down to it.
    pub fn labels(&self, path: &str) -> Vec<String> {
        ancestors(path).iter().map(|p| self.label_of(p)).collect()
    }

    /// Every declared category and the parents it implies.
    pub fn category_paths(&self) -> BTreeSet<String> {
        self.catalogue
            .categories
            .keys()
            .chain(
                self.catalogue
                    .kinds
                    .values()
                    .filter_map(|k| k.category.as_ref()),
            )
            .flat_map(|p| ancestors(p))
            .collect()
    }

    /// A path this source has: declared, implied, or the implicit general.
    pub fn knows_path(&self, path: &str) -> bool {
        path == GENERAL || self.category_paths().contains(path)
    }

    /// Every kind the catalogue or a function names, plus `extra` (received
    /// or muted), sorted.
    pub fn kinds_with<'a>(&self, extra: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
        let mut out: BTreeSet<String> = self
            .catalogue
            .kinds
            .keys()
            .chain(self.declared.keys())
            .cloned()
            .collect();
        out.extend(extra.into_iter().cloned());
        out
    }

    /// The narrowing a `category` filter means, given that a kind the
    /// catalogue does not place is in [`GENERAL`].
    pub fn kinds_in_scope(&self, scope: &str) -> KindSet {
        let placed = self.kinds_with(std::iter::empty());
        let (inside, outside): (Vec<String>, Vec<String>) = placed
            .into_iter()
            .partition(|k| is_within(&self.category_of(k), scope));
        if is_within(GENERAL, scope) {
            KindSet::AllBut(outside)
        } else {
            KindSet::Only(inside)
        }
    }

    pub fn find_action(&self, kind: &str, id: &str) -> Option<Action> {
        self.catalogue
            .kinds
            .get(kind)?
            .actions
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }
}

/// Which kinds a category filter keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KindSet {
    /// Exactly these.
    Only(Vec<String>),
    /// Every kind but these — the general subtree, which holds every kind
    /// nobody placed.
    AllBut(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;
    use serde_json::json;

    fn store_ops() -> SourceCatalogue {
        let m = json!({
            "functions": { "lists": { "notify": { "kinds": {
                "list_overdue": {}, "temp_alarm": { "critical": true }, "shift_note": {} } } } },
            "notifications": {
                "categories": { "operations/food-safety": { "label": "Food safety" },
                                "operations": { "label": "Store operations" } },
                "kinds": {
                    "list_overdue": { "category": "operations", "level": "time_sensitive" },
                    "temp_alarm": { "category": "operations/food-safety", "level": "active" }
                }
            }
        });
        SourceCatalogue::app(parse::catalogue(Some(&m)), parse::declared_kinds(Some(&m)))
    }

    #[test]
    fn a_kind_resolves_its_place_and_an_undescribed_one_lands_in_general() {
        let c = store_ops();
        let alarm = c.describe("temp_alarm");
        assert_eq!(
            alarm.category_labels,
            vec!["Store operations", "Food safety"]
        );
        assert_eq!(
            alarm.level,
            Level::Critical,
            "a function's critical: true outranks the catalogue's level"
        );
        let note = c.describe("shift_note");
        assert_eq!(note.category, GENERAL);
        assert_eq!(note.category_labels, vec!["General"]);
        assert_eq!(note.label, "Shift note");
        assert_eq!(note.level, Level::Active);
        assert_eq!(note.channels, vec!["push", "email", "slack"]);
    }

    #[test]
    fn a_category_filter_keeps_its_subtree_and_general_keeps_the_unplaced() {
        let c = store_ops();
        assert_eq!(
            c.kinds_in_scope("operations"),
            KindSet::Only(vec!["list_overdue".into(), "temp_alarm".into()])
        );
        assert_eq!(
            c.kinds_in_scope("operations/food-safety"),
            KindSet::Only(vec!["temp_alarm".into()])
        );
        assert_eq!(
            c.kinds_in_scope(GENERAL),
            KindSet::AllBut(vec!["list_overdue".into(), "temp_alarm".into()])
        );
        assert!(c.knows_path("operations") && c.knows_path(GENERAL));
        assert!(!c.knows_path("operations/other"));
    }
}
