//! The promote notice: when a build that carried a semantic pin goes live,
//! say which semantic views/topics differ between the pin and the revision
//! live will actually read (`workspaces.current_revision_id`).
//!
//! The live channel ignores pins, so a non-empty notice means the model the
//! build was tested against is not (yet) the model it now reads — typically the
//! branch has not been merged and compiled on main. Promote itself is
//! unchanged: this is information on the response, never a gate.

use std::collections::BTreeMap;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

/// Carried on the promote/rollback response when the promoted build had a pin.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SemanticPinNotice {
    /// The revision the build was staged against.
    pub pinned_revision_id: Uuid,
    /// The revision live reads; `None` when the workspace has none promoted.
    pub current_revision_id: Option<Uuid>,
    /// Views whose name exists on one side only or whose definition differs.
    pub views_differ: Vec<String>,
    pub topics_differ: Vec<String>,
    pub message: String,
}

/// `name → (definition, blob key)`. Both halves compared: with S3 blob
/// storage on, the body's identity is the content-addressed key.
type Defs = BTreeMap<String, (serde_json::Value, Option<String>)>;

/// The notice for promoting `build_id`, or `None` when it carried no pin (or
/// the lookups failed — a notice is advisory and never fails a promote).
pub async fn pin_drift(db: &DatabaseConnection, build_id: Uuid) -> Option<SemanticPinNotice> {
    let build = entity::app_builds::Entity::find_by_id(build_id)
        .one(db)
        .await
        .ok()??;
    let pinned = build.semantic_revision_id?;
    let app = entity::apps::Entity::find_by_id(build.app_id)
        .one(db)
        .await
        .ok()??;
    let current = entity::workspaces::Entity::find_by_id(app.project_id)
        .one(db)
        .await
        .ok()??
        .current_revision_id;
    let (pin_views, pin_topics) = load_defs(db, pinned).await?;
    let (cur_views, cur_topics) = match current {
        Some(rev) => load_defs(db, rev).await?,
        None => (Defs::new(), Defs::new()),
    };
    let views_differ = differing(&pin_views, &cur_views);
    let topics_differ = differing(&pin_topics, &cur_topics);
    let message = notice_message(&views_differ, &topics_differ);
    Some(SemanticPinNotice {
        pinned_revision_id: pinned,
        current_revision_id: current,
        views_differ,
        topics_differ,
        message,
    })
}

async fn load_defs(db: &DatabaseConnection, revision_id: Uuid) -> Option<(Defs, Defs)> {
    let views = entity::semantic_views::Entity::find()
        .filter(entity::semantic_views::Column::RevisionId.eq(revision_id))
        .all(db)
        .await
        .ok()?
        .into_iter()
        .map(|v| (v.name, (v.definition, v.compiled_sql_blob_key)))
        .collect();
    let topics = entity::semantic_topics::Entity::find()
        .filter(entity::semantic_topics::Column::RevisionId.eq(revision_id))
        .all(db)
        .await
        .ok()?
        .into_iter()
        .map(|t| (t.name, (t.definition, t.compiled_sql_blob_key)))
        .collect();
    Some((views, topics))
}

/// Names present on one side only, or on both with a different body. Sorted.
pub(crate) fn differing(a: &Defs, b: &Defs) -> Vec<String> {
    let mut out: Vec<String> = a
        .iter()
        .filter(|(name, body)| b.get(*name) != Some(*body))
        .map(|(name, _)| name.clone())
        .chain(b.keys().filter(|name| !a.contains_key(*name)).cloned())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn notice_message(views: &[String], topics: &[String]) -> String {
    if views.is_empty() && topics.is_empty() {
        return "this build was staged against a pinned semantic revision; the live revision has \
                the same views and topics"
            .to_string();
    }
    format!(
        "this build was staged against a pinned semantic revision that differs from what live \
         reads ({} view(s), {} topic(s)). Live ignores the pin — merge the branch so main \
         compiles and promotes, or the app will read the current model",
        views.len(),
        topics.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defs(entries: &[(&str, serde_json::Value)]) -> Defs {
        entries
            .iter()
            .map(|(n, v)| (n.to_string(), (v.clone(), None)))
            .collect()
    }

    #[test]
    fn changed_added_and_removed_names_all_differ() {
        let pin = defs(&[("orders", json!({"a": 1})), ("new_view", json!({}))]);
        let cur = defs(&[("orders", json!({"a": 2})), ("gone", json!({}))]);
        assert_eq!(differing(&pin, &cur), vec!["gone", "new_view", "orders"]);
    }

    #[test]
    fn identical_models_differ_nowhere() {
        let a = defs(&[("orders", json!({"a": 1}))]);
        assert!(differing(&a, &a.clone()).is_empty());
        assert!(notice_message(&[], &[]).contains("same views"));
    }
}
