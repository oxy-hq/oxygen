//! One source's tree as `GET /api/notifications/catalog` answers it: its
//! categories, recursively, each with its kinds, the person's mutes at that
//! node and the unread count of its subtree.
//!
//! Pure: everything a person has (received, muted, unread) comes in as
//! [`Facts`], so the shape is testable without a database.

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::model::{ActionView, GENERAL, Level, ancestors, parent};
use super::source::{KindInfo, SourceCatalogue};
use crate::settings::model::{Node, NodeSettings};

#[derive(Debug, Clone, Serialize)]
pub struct CategoryNode {
    pub path: String,
    pub label: String,
    pub description: Option<String>,
    /// Mutes set at this node; a parent's are not repeated.
    pub muted: Vec<&'static str>,
    pub unread: u64,
    pub categories: Vec<CategoryNode>,
    pub kinds: Vec<KindNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KindNode {
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
    pub level: Level,
    /// `level == critical`: delivered whatever is muted.
    pub critical: bool,
    pub channels: Vec<&'static str>,
    pub muted: Vec<&'static str>,
    pub actions: Vec<ActionView>,
    pub unread: u64,
}

/// What one person has from one source.
#[derive(Debug, Default)]
pub struct Facts {
    /// Kinds received or set, beyond what the catalogue names.
    pub kinds: BTreeSet<String>,
    pub unread: HashMap<String, u64>,
    /// What the person set at each node of this source
    /// (`notification_node_settings`).
    pub nodes: HashMap<Node, NodeSettings>,
}

impl Facts {
    /// What is set at `node`; nothing when unset.
    pub fn at(&self, node: &Node) -> NodeSettings {
        self.nodes.get(node).cloned().unwrap_or_default()
    }

    /// The channels turned off at `scope` (`""`: the whole source), in
    /// order. The catalogue's `muted`.
    pub fn scope_muted(&self, scope: &str) -> Vec<&'static str> {
        self.at(&Node::Scope(scope.to_string())).off()
    }

    fn kind_muted(&self, kind: &str) -> Vec<&'static str> {
        self.at(&Node::Kind(kind.to_string())).off()
    }
}

/// The kinds a tree draws — those with a switch; a test notice has none
/// (`has_switch`) — and every category path it draws.
pub(crate) fn shape(
    catalogue: &SourceCatalogue,
    facts: &Facts,
) -> (Vec<KindInfo>, BTreeSet<String>) {
    let kinds: Vec<KindInfo> = catalogue
        .kinds_with(&facts.kinds)
        .iter()
        .map(|k| catalogue.describe(k))
        .filter(KindInfo::has_switch)
        .collect();
    let mut paths = catalogue.category_paths();
    for k in &kinds {
        paths.extend(ancestors(&k.category));
    }
    (kinds, paths)
}

/// The top-level categories of `catalogue` for `facts`, and the source's
/// unread total.
pub fn build(catalogue: &SourceCatalogue, facts: &Facts) -> (Vec<CategoryNode>, u64) {
    let (kinds, paths) = shape(catalogue, facts);
    let roots: Vec<CategoryNode> = children(&paths, None)
        .into_iter()
        .map(|p| node(catalogue, facts, &paths, &kinds, &p))
        .collect();
    // Every unread of the source, a kind with no switch included.
    let unread = facts.unread.values().sum();
    (roots, unread)
}

/// The paths directly under `parent_path` (`None`: the top), `general` last.
pub(crate) fn children(paths: &BTreeSet<String>, parent_path: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = paths
        .iter()
        .filter(|p| parent(p) == parent_path)
        .cloned()
        .collect();
    out.sort_by_key(|p| (p == GENERAL, p.clone()));
    out
}

fn node(
    catalogue: &SourceCatalogue,
    facts: &Facts,
    paths: &BTreeSet<String>,
    kinds: &[KindInfo],
    path: &str,
) -> CategoryNode {
    let categories: Vec<CategoryNode> = children(paths, Some(path))
        .into_iter()
        .map(|p| node(catalogue, facts, paths, kinds, &p))
        .collect();
    let mut own: Vec<KindNode> = kinds
        .iter()
        .filter(|k| k.category == path)
        .map(|k| kind_node(facts, k))
        .collect();
    own.sort_by(|a, b| a.label.cmp(&b.label).then(a.kind.cmp(&b.kind)));
    let unread = categories.iter().map(|c| c.unread).sum::<u64>()
        + own.iter().map(|k| k.unread).sum::<u64>();
    CategoryNode {
        path: path.to_string(),
        label: catalogue.label_of(path),
        description: catalogue.description_of(path),
        muted: facts.scope_muted(path),
        unread,
        categories,
        kinds: own,
    }
}

fn kind_node(facts: &Facts, k: &KindInfo) -> KindNode {
    KindNode {
        kind: k.kind.clone(),
        label: k.label.clone(),
        description: k.description.clone(),
        level: k.level,
        critical: k.critical(),
        channels: k.channels.clone(),
        muted: facts.kind_muted(&k.kind),
        actions: k.actions.iter().map(ActionView::from).collect(),
        unread: facts.unread.get(&k.kind).copied().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::live::from_manifest;
    use crate::delivery::Channel;
    use serde_json::json;

    fn store_ops() -> SourceCatalogue {
        from_manifest(Some(&json!({
            "functions": { "f": { "notify": { "kinds": { "list_overdue": {}, "temp_alarm": {} } } } },
            "notifications": {
                "categories": {
                    "operations": { "label": "Store operations" },
                    "operations/food-safety": { "label": "Food safety", "description": "Temps" }
                },
                "kinds": {
                    "list_overdue": { "category": "operations", "label": "Overdue lists",
                                      "actions": [{ "id": "open", "label": "Open list" }] },
                    "temp_alarm": { "category": "operations/food-safety", "level": "critical",
                                    "actions": [{ "id": "ack", "label": "Ack", "function": "f" }] }
                }
            }
        })))
    }

    #[test]
    fn the_tree_nests_categories_counts_unread_and_keeps_mutes_where_they_were_set() {
        let off = |channels: &[Channel]| {
            let mut s = NodeSettings::default();
            for c in channels {
                s.set_channel(*c, Some(false));
            }
            s
        };
        let mut on_push = off(&[Channel::Email]);
        on_push.set_channel(Channel::Push, Some(true));
        let facts = Facts {
            kinds: ["old_kind".to_string()].into(),
            unread: [("temp_alarm".into(), 2), ("old_kind".into(), 1)].into(),
            nodes: [
                (
                    Node::Kind("list_overdue".into()),
                    off(&[Channel::Slack, Channel::Push]),
                ),
                (Node::Scope("operations".into()), on_push),
                (Node::source(), off(&[Channel::Slack])),
            ]
            .into(),
        };
        let (roots, unread) = build(&store_ops(), &facts);
        assert_eq!(unread, 3);
        let paths: Vec<_> = roots.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["operations", "general"], "general goes last");

        let ops = &roots[0];
        assert_eq!((ops.label.as_str(), ops.unread), ("Store operations", 2));
        assert_eq!(ops.muted, vec!["email"], "an explicit on is not a mute");
        assert_eq!(ops.kinds[0].muted, vec!["push", "slack"], "channel order");
        let safety = &ops.categories[0];
        assert_eq!(safety.path, "operations/food-safety");
        assert_eq!(safety.description.as_deref(), Some("Temps"));
        assert!(safety.muted.is_empty(), "a parent's mute is not repeated");
        let alarm = &safety.kinds[0];
        assert!(alarm.critical);
        assert!(alarm.actions[0].runs_function);

        let general = &roots[1];
        assert_eq!(general.label, "General");
        assert_eq!(general.kinds[0].kind, "old_kind");
        assert_eq!(general.kinds[0].label, "Old kind");
        assert_eq!(general.unread, 1);
    }

    #[test]
    fn every_field_is_serialized_null_not_missing() {
        let (roots, _) = build(&store_ops(), &Facts::default());
        let v = serde_json::to_value(&roots[0]).unwrap();
        assert_eq!(v["description"], serde_json::Value::Null);
        assert_eq!(v["kinds"][0]["description"], serde_json::Value::Null);
        assert_eq!(v["kinds"][0]["level"], "active");
        assert_eq!(
            v["kinds"][0]["actions"][0],
            json!({ "id": "open", "label": "Open list", "destructive": false, "runs_function": false })
        );
    }
}
