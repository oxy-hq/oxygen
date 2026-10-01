//! What the change check can find, and how bad each finding is.

use serde::{Deserialize, Serialize};

/// How a branch's `.airway.yml` change lands on the live tables once merged.
///
/// Ordered by severity, so a pipeline's verdict is the `max` of its findings'.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Airway's own additive evolution absorbs it: new tables, new columns,
    /// config that changes no stored shape.
    Additive,
    /// Merges cleanly, but something is left behind or could not be checked.
    Warning,
    /// Airway's schema migration is additive only, so this change needs an
    /// explicit **Reset schema** after merge (and usually a backfill).
    NeedsReset,
}

/// One thing the check noticed about a pipeline.
///
/// `resource` names what the connector advertises (`ResourceInfo.name`), or —
/// on [`Finding::TableRenamed`] — the normalizer output name a
/// `table_name_mappings` entry is keyed by. `table` names a destination table.
///
/// Serialize only: `ConfigOnly.field` is a `&'static str` naming a spec field,
/// and the check's consumers read the result as JSON rather than back into
/// this type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "finding", rename_all = "snake_case")]
pub enum Finding {
    ResourceAdded {
        resource: String,
    },
    TableAdded {
        table: String,
    },
    /// A new column on an existing table. Airway adds it on the next load.
    ColumnAdded {
        table: String,
        column: String,
    },
    /// A spec field that changes no stored shape: `description`,
    /// `concurrency`, `streaming`, `channel_capacity`, `allow_concurrent_runs`,
    /// `source.config` (with the same `kind`), or a narrower `resources:` list.
    ConfigOnly {
        field: &'static str,
    },
    /// `name` changed: a new state key and an empty cursor, so `append` tables
    /// re-extract history into the old tables.
    PipelineRenamed {
        from: String,
        to: String,
    },
    /// `database` or `dataset_name` changed. `from`/`to` read `database.dataset`.
    DestinationMoved {
        from: String,
        to: String,
    },
    SchemaSeparatorChanged {
        from: Option<String>,
        to: Option<String>,
    },
    SourceKindChanged {
        from: String,
        to: String,
    },
    WriteDispositionChanged {
        resource: String,
        from: String,
        to: String,
    },
    /// The key a `merge` or `replacing` table collapses on changed.
    PrimaryKeyChanged {
        resource: String,
        from: Option<Vec<String>>,
        to: Option<Vec<String>>,
    },
    TableRenamed {
        resource: String,
        from: String,
        to: String,
    },
    /// Airway would otherwise open a `<column>__v_<type>` variant column.
    ColumnTypeChanged {
        table: String,
        column: String,
        live: String,
        candidate: String,
    },
    /// A column the live table has that the candidate schema no longer
    /// produces. Airway keeps the column; it stops receiving values.
    ColumnRemoved {
        table: String,
        column: String,
    },
    ResourceRemoved {
        resource: String,
    },
    PipelineRemoved {
        name: String,
    },
    /// The stored schema lists a table Airhouse does not have.
    LiveDrift {
        table: String,
    },
    Unevaluated {
        reason: String,
    },
}

impl Finding {
    pub fn verdict(&self) -> Verdict {
        match self {
            Finding::ResourceAdded { .. }
            | Finding::TableAdded { .. }
            | Finding::ColumnAdded { .. }
            | Finding::ConfigOnly { .. } => Verdict::Additive,
            Finding::PipelineRenamed { .. }
            | Finding::DestinationMoved { .. }
            | Finding::SchemaSeparatorChanged { .. }
            | Finding::SourceKindChanged { .. }
            | Finding::WriteDispositionChanged { .. }
            | Finding::PrimaryKeyChanged { .. }
            | Finding::TableRenamed { .. }
            | Finding::ColumnTypeChanged { .. } => Verdict::NeedsReset,
            Finding::ColumnRemoved { .. }
            | Finding::ResourceRemoved { .. }
            | Finding::PipelineRemoved { .. }
            | Finding::LiveDrift { .. }
            | Finding::Unevaluated { .. } => Verdict::Warning,
        }
    }

    /// What production needs after merge, in the words the Previews tab shows.
    /// `None` for an additive finding: nothing to do.
    pub fn prod_action(&self) -> Option<&'static str> {
        Some(match self {
            Finding::ResourceAdded { .. }
            | Finding::TableAdded { .. }
            | Finding::ColumnAdded { .. }
            | Finding::ConfigOnly { .. } => return None,
            Finding::PipelineRenamed { .. } => "Reset schema, or keep the old name",
            Finding::DestinationMoved { .. } => {
                "Reset schema on the old dataset; the new one starts empty"
            }
            Finding::SchemaSeparatorChanged { .. }
            | Finding::SourceKindChanged { .. }
            | Finding::TableRenamed { .. }
            | Finding::ColumnTypeChanged { .. } => "Reset schema",
            Finding::WriteDispositionChanged { .. } | Finding::PrimaryKeyChanged { .. } => {
                "Reset schema, then backfill"
            }
            Finding::ColumnRemoved { .. } => {
                "The live column is left behind and stops receiving values"
            }
            Finding::ResourceRemoved { .. } | Finding::PipelineRemoved { .. } => {
                "The live table is left behind, not dropped"
            }
            Finding::LiveDrift { .. } => "Investigate before merging",
            Finding::Unevaluated { .. } => "Check this pipeline by hand before merging",
        })
    }
}
