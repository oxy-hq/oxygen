//! A reported run's headline at a glance: the few fields of its stored report a
//! list row reads to sum up the result (`ExperimentListItem::report_glance`).
//!
//! Read only for a run whose state is `reported` — the same gate as
//! [`super::results_api`]: a results row that exists is not a licence to show it.

use std::collections::HashMap;

use entity::experiment_results;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use serde_json::{Map, Value};
use uuid::Uuid;

/// What the row formats and colours the effect by, as the report page does.
const KEYS: &[&str] = &[
    "status",
    "design",
    "grain",
    "share",
    "proportional",
    "directional",
    "alpha",
    "refusal",
];
const HEADLINE_KEYS: &[&str] = &["estimate", "significant", "verdict"];

/// The glance of each run in `reported_run_ids` that has a results row, in one query.
pub async fn glances_of(
    db: &DatabaseConnection,
    reported_run_ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, DbErr> {
    if reported_run_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, Value)> = experiment_results::Entity::find()
        .select_only()
        .columns([
            experiment_results::Column::RunId,
            experiment_results::Column::Report,
        ])
        .filter(experiment_results::Column::RunId.is_in(reported_run_ids.to_vec()))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(run_id, report)| (run_id, pick(&report)))
        .collect())
}

/// The keys the row reads, as stored; a key the report lacks stays absent. The
/// headline is always present, `null` when the report has none (a refusal).
pub fn pick(report: &Value) -> Value {
    let mut out = pick_keys(report, KEYS);
    let headline = match report.get("headline") {
        Some(h) if h.is_object() => Value::Object(pick_keys(h, HEADLINE_KEYS)),
        _ => Value::Null,
    };
    out.insert("headline".into(), headline);
    Value::Object(out)
}

fn pick_keys(v: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .filter_map(|k| v.get(*k).map(|x| ((*k).to_string(), x.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_glance_keeps_only_what_the_row_reads() {
        let report = json!({
            "status": { "kind": "registered" }, "design": "groups", "directional": true,
            "alpha": 0.2, "refusal": null, "seeds": { "assignment": 7 },
            "headline": { "estimate": 3.5, "significant": true, "verdict": "directional", "text": "…" }
        });
        assert_eq!(
            pick(&report),
            json!({
                "status": { "kind": "registered" }, "design": "groups", "directional": true,
                "alpha": 0.2, "refusal": null,
                "headline": { "estimate": 3.5, "significant": true, "verdict": "directional" }
            })
        );
    }

    #[test]
    fn a_refused_report_has_a_null_headline() {
        let v = pick(&json!({ "refusal": "too few units", "headline": null }));
        assert_eq!(v, json!({ "refusal": "too few units", "headline": null }));
        assert_eq!(pick(&json!({}))["headline"], Value::Null);
    }
}
