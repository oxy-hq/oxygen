//! Schema-to-schema comparison: a live stored schema against a candidate one
//! (a preview sample's, once samples exist).

use std::collections::BTreeSet;

use airway::schema::evolution::{SchemaChange, diff_schemas};
use airway::schema::{Schema, Table};

use super::{Finding, disposition_label, key_sensitive, normalized_key};

/// Everything `candidate` would change about `live`.
///
/// Additions come from airway's own [`diff_schemas`], so this agrees with
/// what the engine would evolve. `diff_schemas` only looks one way — it never
/// reports a column the candidate lost, a column whose type changed in place,
/// or a changed primary key — so those are checked here.
///
/// A table in `live` that `candidate` lacks is **not** a finding: a candidate
/// may cover a subset of resources (a bounded sample), and airway leaves
/// tables it did not load alone.
///
/// Sorted, so an unchanged pair of schemas always yields the same list.
pub fn compare_schemas(live: &Schema, candidate: &Schema) -> Vec<Finding> {
    let mut findings: Vec<Finding> = diff_schemas(live, candidate)
        .into_iter()
        .filter_map(|change| from_change(live, change))
        .collect();

    let shared: BTreeSet<&String> = live
        .tables
        .keys()
        .filter(|t| candidate.tables.contains_key(*t))
        .collect();
    for table in shared {
        compare_tables(
            table,
            &live.tables[table],
            &candidate.tables[table],
            &mut findings,
        );
    }

    findings.sort_by_key(|f| format!("{f:?}"));
    findings.dedup();
    findings
}

/// Translate one airway schema change into a finding.
fn from_change(live: &Schema, change: SchemaChange) -> Option<Finding> {
    Some(match change {
        SchemaChange::TableAdded { table_name } => Finding::TableAdded { table: table_name },
        SchemaChange::ColumnAdded { table_name, column } => Finding::ColumnAdded {
            table: table_name,
            column: column.name,
        },
        SchemaChange::VariantColumnCreated {
            table_name,
            original_column,
            new_type,
            ..
        } => {
            let live_type = live
                .tables
                .get(&table_name)
                .and_then(|t| t.columns.get(&original_column))
                .map(|c| c.data_type.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            Finding::ColumnTypeChanged {
                table: table_name,
                column: original_column,
                live: live_type,
                candidate: new_type.to_string(),
            }
        }
        SchemaChange::WriteDispositionChanged {
            table_name,
            old,
            new,
        } => Finding::WriteDispositionChanged {
            resource: table_name,
            from: disposition_label(&old),
            to: disposition_label(&new),
        },
    })
}

/// The checks `diff_schemas` does not make, for one table both sides have.
fn compare_tables(name: &str, live: &Table, candidate: &Table, findings: &mut Vec<Finding>) {
    for (column, live_col) in &live.columns {
        match candidate.columns.get(column) {
            None => findings.push(Finding::ColumnRemoved {
                table: name.to_string(),
                column: column.clone(),
            }),
            Some(cand_col) if cand_col.data_type != live_col.data_type => {
                findings.push(Finding::ColumnTypeChanged {
                    table: name.to_string(),
                    column: column.clone(),
                    live: live_col.data_type.to_string(),
                    candidate: cand_col.data_type.to_string(),
                })
            }
            Some(_) => {}
        }
    }

    let keyed =
        key_sensitive(&live.write_disposition) || key_sensitive(&candidate.write_disposition);
    let live_pk = table_key(live);
    let cand_pk = table_key(candidate);
    if keyed && live_pk != cand_pk {
        findings.push(Finding::PrimaryKeyChanged {
            resource: name.to_string(),
            from: live_pk,
            to: cand_pk,
        });
    }
}

fn table_key(table: &Table) -> Option<Vec<String>> {
    normalized_key(Some(
        table
            .primary_keys()
            .into_iter()
            .map(str::to_string)
            .collect(),
    ))
}
