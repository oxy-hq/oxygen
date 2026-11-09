//! Does a lever automation read the units it is sent? An automation that never
//! uses `{{ unit }}` (`per: unit`) or `{{ units }}` (`per: wave`) — or, under
//! `assign_by`, `{{ members }}` — moves the same thing whichever unit it is
//! handed, so each per-unit flip would switch the whole fleet. Pricing and
//! registration refuse such a lever (via [`super::lever::check_levers`]).
//!
//! A reference is read from the compiled definition: every Jinja string in it,
//! plus the body of each `sql_file` it names (the body renders against the same
//! variables). A `sql_file` whose body cannot be read at the lever's revision —
//! its path is itself a template, or it is not compiled there — means the
//! check cannot see every template, so it does not refuse.

use std::collections::BTreeSet;

use entity::{automation_definitions, verified_queries};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::config::LeverPer;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;

use super::lever::ResolvedLever;

/// The variables a lever may read to know which units it moves: the first is
/// the one the refusal names.
pub fn unit_variables(per: LeverPer, grouped: bool) -> Vec<&'static str> {
    let mut wanted = vec![match per {
        LeverPer::Unit => "unit",
        LeverPer::Wave => "units",
    }];
    if grouped {
        wanted.push("members");
    }
    wanted
}

/// Every top-level variable the definition's Jinja strings read. A string that
/// does not parse reads nothing: rendering it would fail before any flip.
pub fn template_variables(definition: &Value) -> BTreeSet<String> {
    let env = minijinja::Environment::new();
    let mut read = BTreeSet::new();
    visit_strings(definition, &mut |s| {
        if !(s.contains("{{") || s.contains("{%")) {
            return;
        }
        if let Ok(template) = env.template_from_str(s) {
            read.extend(template.undeclared_variables(false));
        }
    });
    read
}

/// The `sql_file` refs the definition names, `./` stripped. A templated path
/// is returned as-is: it cannot be looked up, which the caller treats as unread.
pub fn sql_files(definition: &Value) -> Vec<String> {
    let mut files = Vec::new();
    collect_sql_files(definition, &mut files);
    files
}

fn collect_sql_files(value: &Value, files: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                match (key.as_str(), v.as_str()) {
                    ("sql_file", Some(path)) => {
                        files.push(path.trim_start_matches("./").to_string())
                    }
                    _ => collect_sql_files(v, files),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_sql_files(v, files)),
        _ => {}
    }
}

fn visit_strings(value: &Value, f: &mut impl FnMut(&str)) {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter().for_each(|v| visit_strings(v, f)),
        Value::Object(map) => map.values().for_each(|v| visit_strings(v, f)),
        _ => {}
    }
}

/// The refusal for an automation that reads none of `wanted`, `None` when it
/// reads one.
pub fn verdict(
    via: &str,
    file_path: &str,
    read: &BTreeSet<String>,
    wanted: &[&str],
) -> Option<String> {
    if wanted.iter().any(|w| read.contains(*w)) {
        return None;
    }
    let named = wanted
        .iter()
        .map(|w| format!("`{{{{ {w} }}}}`"))
        .collect::<Vec<_>>()
        .join(" or ");
    Some(format!(
        "automation `{via}` ({file_path}) never uses {named}, so it would do the same thing \
         whichever units it is sent: each flip would switch the whole fleet"
    ))
}

/// The refusal for `lever`, or `None` when its automation reads a unit
/// variable (or when a `sql_file` body could not be read).
pub async fn unit_reference_refusal(
    db: &impl ConnectionTrait,
    cfg: &ExperimentConfig,
    via: &str,
    lever: &ResolvedLever,
) -> Result<Option<String>, DbErr> {
    let Some(row) =
        automation_definitions::Entity::find_by_id((lever.revision_id, lever.file_path.clone()))
            .one(db)
            .await?
    else {
        return Ok(None);
    };
    let wanted = unit_variables(cfg.lever_per(), cfg.assign_by.is_some());
    let mut read = template_variables(&row.definition);
    if wanted.iter().any(|w| read.contains(*w)) {
        return Ok(None);
    }
    let mut files = sql_files(&row.definition);
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Ok(verdict(via, &lever.file_path, &read, &wanted));
    }
    let bodies = verified_queries::Entity::find()
        .filter(verified_queries::Column::RevisionId.eq(lever.revision_id))
        .filter(verified_queries::Column::FilePath.is_in(files.clone()))
        .all(db)
        .await?;
    if bodies.len() < files.len() {
        return Ok(None);
    }
    for body in bodies {
        read.extend(template_variables(&Value::String(body.content)));
    }
    Ok(verdict(via, &lever.file_path, &read, &wanted))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn read(definition: &Value) -> Vec<String> {
        template_variables(definition).into_iter().collect()
    }

    #[test]
    fn a_per_unit_lever_reads_unit_and_a_grouped_one_may_read_members() {
        assert_eq!(unit_variables(LeverPer::Unit, false), ["unit"]);
        assert_eq!(unit_variables(LeverPer::Wave, false), ["units"]);
        assert_eq!(unit_variables(LeverPer::Unit, true), ["unit", "members"]);
        assert_eq!(unit_variables(LeverPer::Wave, true), ["units", "members"]);
    }

    #[test]
    fn variables_are_read_from_every_jinja_string_at_any_depth() {
        let def = json!({
            "name": "promo_on",
            "tasks": [
                { "type": "execute_sql", "sql_query": "UPDATE s SET on = true WHERE id = '{{ unit | sqlquote }}'" },
                { "type": "loop_sequential", "values": "{{ units }}", "tasks": [
                    { "type": "execute_sql", "sql_query": "SELECT '{{ loop.value }}', {{ wave }}" }
                ]},
            ],
        });
        assert_eq!(read(&def), ["loop", "unit", "units", "wave"]);
    }

    #[test]
    fn a_variable_the_template_binds_itself_is_not_a_reference() {
        let def = json!({ "sql_query": "{% for unit in ['a'] %}{{ unit }}{% endfor %}{% set units = 1 %}{{ units }}" });
        assert!(read(&def).is_empty(), "{:?}", read(&def));
    }

    #[test]
    fn plain_strings_and_unparsable_templates_read_nothing() {
        let def =
            json!({ "description": "turns the promo on for {{ unit", "sql_query": "SELECT 1" });
        assert!(read(&def).is_empty());
    }

    #[test]
    fn a_declared_but_unused_variable_is_not_a_reference() {
        let def = json!({
            "variables": { "unit": { "default": "" } },
            "tasks": [{ "type": "execute_sql", "sql_query": "UPDATE promo SET on = true" }],
        });
        assert!(read(&def).is_empty());
    }

    #[test]
    fn sql_files_are_collected_with_the_leading_dot_slash_stripped() {
        let def = json!({ "tasks": [
            { "type": "execute_sql", "sql_file": "./sql/flip.sql" },
            { "type": "loop_sequential", "tasks": [{ "sql_file": "sql/{{ state }}.sql" }] },
        ]});
        assert_eq!(sql_files(&def), ["sql/flip.sql", "sql/{{ state }}.sql"]);
    }

    #[test]
    fn a_lever_that_reads_a_wanted_variable_passes() {
        let got: BTreeSet<String> = ["state".into(), "members".into()].into();
        assert_eq!(
            verdict("promo_on", "a.yml", &got, &["unit", "members"]),
            None
        );
    }

    #[test]
    fn a_lever_that_ignores_the_unit_is_refused_in_plain_words() {
        let got: BTreeSet<String> = ["state".into()].into();
        let message = verdict(
            "promo_on",
            "automations/promo_on.automation.yml",
            &got,
            &["unit"],
        )
        .expect("refused");
        assert!(message.contains("`promo_on`"), "{message}");
        assert!(
            message.contains("automations/promo_on.automation.yml"),
            "{message}"
        );
        assert!(message.contains("`{{ unit }}`"), "{message}");
        assert!(message.contains("whole fleet"), "{message}");
    }

    #[test]
    fn a_grouped_refusal_names_both_variables_it_would_accept() {
        let message =
            verdict("promo_on", "a.yml", &BTreeSet::new(), &["units", "members"]).expect("refused");
        assert!(
            message.contains("`{{ units }}`") && message.contains("`{{ members }}`"),
            "{message}"
        );
    }
}
