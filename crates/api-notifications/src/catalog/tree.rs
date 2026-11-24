//! One source's tree as `GET /api/notifications/catalog` answers it: its
//! categories, recursively, each with its kinds, the person's mutes at that
//! node and the unread count of its subtree.
//!
//! Pure: everything a person has (received, muted, unread) comes in as
//! [`Facts`], and what they stand as comes in as a
//! [`Standing`](crate::operators::Standing), so the shape is testable without a
//! database.
//!
//! A kind is drawn when **its audience admits them by standing, or they have
//! received or set it** — `preferences::may_be_told` asked per kind instead of
//! per source. The second half is what keeps the change from losing anything: a
//! notice already in someone's inbox always has a switch, whatever its
//! audience. A category with nothing left under it is not drawn at all, so a
//! crew member sees no empty "Billing".

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::model::{ActionView, GENERAL, Level, ancestors, parent};
use super::source::{KindInfo, SourceCatalogue};
use crate::operators::Standing;
use crate::preferences::may_be_told;
use crate::settings::model::{Node, NodeSettings};
use crate::stream::Stream;

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
    /// Which stream the inbox files it under (`crate::stream`). The page uses
    /// it to show one stream's categories at a time.
    pub stream: &'static str,
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

/// The kinds a tree draws — those with a switch (a test notice has none,
/// `has_switch`) and whose audience this person is in — and every category path
/// it draws.
///
/// The paths come from the surviving kinds alone, never from the declared
/// categories: that is the pruning. A category whose kinds are all filtered out
/// disappears instead of being drawn empty, and a category declared with no
/// kinds at all — Oxygen's reserved `apps` — draws nothing until it has one.
pub(crate) fn shape(
    catalogue: &SourceCatalogue,
    facts: &Facts,
    standing: Standing,
) -> (Vec<KindInfo>, BTreeSet<String>) {
    let kinds: Vec<KindInfo> = catalogue
        .kinds_with(&facts.kinds)
        .iter()
        .map(|k| catalogue.describe(k))
        .filter(KindInfo::has_switch)
        .filter(|k| {
            // Identical to the rule that decides whether an app may reach
            // someone, asked of one kind: standing admits them, or they have
            // already been told.
            may_be_told(facts.kinds.contains(&k.kind), standing.admits(k.audience))
        })
        .collect();
    let mut paths = BTreeSet::new();
    for k in &kinds {
        paths.extend(ancestors(&k.category));
    }
    (kinds, paths)
}

/// The top-level categories of `catalogue` for `facts`, and the source's
/// unread total.
pub fn build(
    catalogue: &SourceCatalogue,
    facts: &Facts,
    standing: Standing,
) -> (Vec<CategoryNode>, u64) {
    let (kinds, paths) = shape(catalogue, facts, standing);
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
        .map(|k| kind_node(catalogue, facts, k))
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

fn kind_node(catalogue: &SourceCatalogue, facts: &Facts, k: &KindInfo) -> KindNode {
    KindNode {
        stream: Stream::of(catalogue.source, k.audience).as_str(),
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
        let (roots, unread) = build(&store_ops(), &facts, Standing::default());
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

    /// Oxygen's own tree, as one person sees it.
    fn oxygen_tree(facts: &Facts, standing: Standing) -> Vec<CategoryNode> {
        build(&crate::catalog::platform::oxygen(), facts, standing).0
    }

    fn paths_of(roots: &[CategoryNode]) -> Vec<String> {
        roots
            .iter()
            .flat_map(|r| std::iter::once(r.path.clone()).chain(paths_of(&r.categories)))
            .collect()
    }

    fn received(kinds: &[&str]) -> Facts {
        Facts {
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_crew_member_who_got_one_workspace_ready_sees_workspaces_and_nothing_else() {
        // The bug this exists to fix: one legitimate "your apps are ready"
        // used to unlock Billing, Automations and pipelines, Monitoring and
        // Access switches for good.
        let roots = oxygen_tree(
            &received(&[crate::platform::kinds::WORKSPACE_READY]),
            Standing::default(),
        );
        assert_eq!(paths_of(&roots), vec!["workspaces"]);
    }

    #[test]
    fn an_addressed_category_appears_the_moment_its_notice_arrives_and_not_before() {
        use crate::platform::kinds::{ANOMALY, ASK_FINISHED};
        let member = Standing::default();
        assert!(
            !paths_of(&oxygen_tree(&received(&[]), member)).contains(&"monitoring".to_string())
        );
        // An anomaly names the people it is for, whatever their role — so the
        // switch they need appears with the first one, by the received clause.
        let roots = oxygen_tree(&received(&[ANOMALY, ASK_FINISHED]), member);
        let paths = paths_of(&roots);
        assert!(paths.contains(&"monitoring".to_string()) && paths.contains(&"ask".to_string()));
        assert!(!paths.contains(&"billing".to_string()), "still not theirs");
    }

    #[test]
    fn a_plain_member_keeps_an_operator_notice_they_already_have() {
        // Nobody loses sight of something already in their inbox: the switch
        // for it must stay reachable or they cannot turn it off.
        let roots = oxygen_tree(
            &received(&[crate::platform::kinds::RUN_FAILED]),
            Standing::default(),
        );
        // Beside Workspaces, which every member sees: `workspace_ready` is
        // `Everyone`, so its category is drawn whether or not one has arrived.
        assert_eq!(paths_of(&roots), vec!["runs", "workspaces"]);
        assert_eq!(roots[0].kinds[0].stream, "operations");
    }

    #[test]
    fn an_operator_sees_the_whole_tree_and_so_does_unknown_standing() {
        let operator = Standing {
            operator: true,
            officer: true,
            owner: true,
        };
        for standing in [operator, Standing::EVERYTHING] {
            let paths = paths_of(&oxygen_tree(&received(&[]), standing));
            assert!(
                paths.contains(&"billing".to_string()) && paths.contains(&"runs".to_string()),
                "{standing:?} sees the operator categories"
            );
            // Addressed kinds are still not drawn by standing alone.
            assert!(!paths.contains(&"monitoring".to_string()), "{standing:?}");
        }
    }

    #[test]
    fn an_org_admin_who_is_not_an_owner_gets_automations_and_not_billing() {
        // `billing_past_due` goes to owners only, so an admin seeing a Billing
        // switch would be a switch over a notice they can never receive.
        let admin = Standing {
            operator: true,
            officer: true,
            owner: false,
        };
        let paths = paths_of(&oxygen_tree(&received(&[]), admin));
        assert_eq!(paths, vec!["runs", "workspaces"], "no billing");
    }

    #[test]
    fn a_kind_carries_the_stream_the_inbox_files_it_under() {
        use crate::platform::kinds::{BILLING_PAST_DUE, RUN_FAILED, WORKSPACE_READY};
        let streams: Vec<(String, &str)> = oxygen_tree(&received(&[]), Standing::EVERYTHING)
            .iter()
            .flat_map(|root| root.kinds.iter())
            .map(|k| (k.kind.clone(), k.stream))
            .collect();
        // The two standing-based kinds are Operations; "your apps are ready" is
        // for everyone, so it is Activity even in an operator's tree.
        assert_eq!(
            streams,
            vec![
                (BILLING_PAST_DUE.to_string(), "operations"),
                (RUN_FAILED.to_string(), "operations"),
                (WORKSPACE_READY.to_string(), "activity"),
            ]
        );
        // And an app's kinds are Activity, whatever the person stands as.
        let app = build(&store_ops(), &Facts::default(), Standing::default()).0;
        assert_eq!(app[0].kinds[0].stream, "activity");
    }

    #[test]
    fn a_kind_you_set_comes_back_and_a_category_you_muted_does_not() {
        use crate::platform::kinds::RUN_FAILED;
        // The two settings are not symmetric, and the asymmetry is in how the
        // facts are built: `shape` keeps a kind the person has in
        // `Facts::kinds`, and `catalog::handler::add_settings` fills that from
        // `Node::Kind` rows alone. So both cases are built here the way the
        // handler builds them — a kind setting lands in both fields, a scope
        // setting in `nodes` only.
        let node = |node: Node| [(node, NodeSettings::default())].into();
        let by_kind = Facts {
            kinds: [RUN_FAILED.to_string()].into(),
            nodes: node(Node::Kind(RUN_FAILED.to_string())),
            ..Default::default()
        };
        assert_eq!(
            paths_of(&oxygen_tree(&by_kind, Standing::default())),
            vec!["runs", "workspaces"],
            "a kind you muted is a kind you are shown: otherwise you could \
             never unmute it"
        );
        let by_scope = Facts {
            nodes: node(Node::Scope("runs".to_string())),
            ..Default::default()
        };
        assert_eq!(
            paths_of(&oxygen_tree(&by_scope, Standing::default())),
            vec!["workspaces"],
            "a category mute is remembered and still applies, but it does not \
             draw a category whose kinds are not this person's"
        );
    }

    #[test]
    fn a_category_with_nothing_left_in_it_is_not_drawn_empty() {
        // The pruning, stated on its own: Oxygen declares a reserved `apps`
        // category with no kinds, and nobody — operator or not — sees it.
        for standing in [Standing::default(), Standing::EVERYTHING] {
            assert!(
                !paths_of(&oxygen_tree(&received(&[]), standing))
                    .contains(&crate::catalog::platform::APPS.to_string()),
                "{standing:?}"
            );
        }
    }

    #[test]
    fn every_field_is_serialized_null_not_missing() {
        let (roots, _) = build(&store_ops(), &Facts::default(), Standing::default());
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
