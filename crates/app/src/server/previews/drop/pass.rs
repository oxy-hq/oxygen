//! The drop itself, pure over what the registry and shadow map said: which
//! schemas this run holds a claim on, whether the preview created each, and
//! which relations it recorded in each.

use std::collections::{BTreeSet, HashMap, HashSet};

use airhouse::preview_sql::PreviewNamespace;

use super::DropReport;
use crate::server::previews::ddl::{
    PreviewDdlError, Relation, RelationKind, SchemaDropper, well_formed,
};

/// One schema the drop run holds a claim on, as its registry row and the
/// preview's shadow map describe it.
#[derive(Debug, Clone, Default)]
pub struct ClaimedSchema {
    pub live_schema: String,
    /// The preview's own strict `CREATE SCHEMA` succeeded
    /// (`schema_created_at`). A schema it never created is never touched.
    pub created: bool,
    /// Lowercase names of the relations the preview recorded in this schema
    /// (shadow-map rows in any state, `dropped` included: they are written
    /// before the step's SQL runs). Only these are dropped.
    ///
    /// The match is by **name only**. Something else that writes a relation of
    /// a recorded name into the preview's schema (a customer's `CREATE OR
    /// REPLACE` there, say) is dropped with it: the schema is the preview's,
    /// and the name is one the preview made. Only an *unrecorded* name marks
    /// the schema as holding someone else's work.
    pub recorded: HashSet<String>,
}

/// What emptying one schema came to.
enum Emptied {
    /// Every relation in it was the preview's; the schema is gone.
    Whole(usize),
    /// The preview's relations are gone, but the schema holds others the
    /// preview did not record, so it stays.
    Kept { dropped: usize, others: Vec<String> },
}

/// Drop each of `requested` that this run holds a claim on (`claimed`), that
/// is well formed for `ns`, and that the preview created, through `dropper`.
/// Only the relations the preview recorded are dropped; a schema holding
/// anything else is kept (`orphaned`).
pub async fn drop_listed(
    dropper: &dyn SchemaDropper,
    ns: &PreviewNamespace,
    claimed: &HashMap<String, ClaimedSchema>,
    requested: &[String],
) -> DropReport {
    let mut report = DropReport::default();
    let unique: BTreeSet<&String> = requested.iter().collect();
    for name in unique {
        let Some(schema) = claimed.get(name) else {
            report
                .refused
                .push((name.clone(), "no registry row claimed by this drop".into()));
            continue;
        };
        if let Err(refused) = well_formed(ns, name) {
            report.refused.push((name.clone(), refused.to_string()));
            continue;
        }
        if !schema.created {
            report.never_created.push(name.clone());
            continue;
        }
        match drop_one(dropper, name, &schema.recorded).await {
            Ok(Emptied::Whole(relations)) => report.dropped.push((name.clone(), relations)),
            Ok(Emptied::Kept { dropped, others }) => {
                report.orphaned.push((name.clone(), dropped, others))
            }
            Err(e) => report.failed.push((name.clone(), e.to_string())),
        }
    }
    report
}

/// Drop the relations of `schema` the preview recorded (views first), and the
/// schema itself when nothing else is in it.
async fn drop_one(
    dropper: &dyn SchemaDropper,
    schema: &str,
    recorded: &HashSet<String>,
) -> Result<Emptied, PreviewDdlError> {
    let (mut ours, others): (Vec<Relation>, Vec<Relation>) = dropper
        .list_relations(schema)
        .await?
        .into_iter()
        .partition(|r| recorded.contains(&r.name.to_ascii_lowercase()));
    ours.sort_by_key(|r| (r.kind != RelationKind::View, r.name.clone()));
    for relation in &ours {
        dropper.drop_relation(schema, relation).await?;
    }
    if !others.is_empty() {
        let mut others: Vec<String> = others.into_iter().map(|r| r.name).collect();
        others.sort();
        return Ok(Emptied::Kept {
            dropped: ours.len(),
            others,
        });
    }
    dropper.drop_schema(schema).await?;
    Ok(Emptied::Whole(ours.len()))
}
