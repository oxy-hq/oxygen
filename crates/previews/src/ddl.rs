//! The port through which a preview creates and drops its own Airhouse
//! schemas, and the only statements that port may send.
//!
//! A preview's schemas are `preview_<key>__<live schema>`
//! ([`PreviewNamespace`]). Nothing here takes SQL from a caller: [`Ddl`] builds
//! one of a few fixed statements, and only from a name that is
//! [`well_formed`] for the preview's namespace. Whether a name may be created
//! or dropped at all is the registry's call (`previews::registry`,
//! `previews::drop`); this module is the last fence, so a bug upstream still
//! cannot turn into `DROP SCHEMA <customer schema>`.
//!
//! Two implementations: [`super::ddl_airhouse::AirhousePreviewDdl`] (a system
//! Writer on the workspace's Airhouse) and [`super::ddl_duckdb::DuckDbPreviewDdl`]
//! (in-process DuckDB, the stand-in tests drive).

use std::sync::LazyLock;

use airhouse::preview_sql::{PreviewNamespace, Refused};
use async_trait::async_trait;
use regex::Regex;
use serde::Serialize;
use uuid::Uuid;

/// Every name a preview schema can have, independent of which preview.
static PREVIEW_SCHEMA_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^preview_[a-z0-9_]{1,24}_[0-9a-f]{6}__[a-z0-9_]+$").expect("static regex")
});

/// A relation inside a preview schema, as `information_schema.tables` lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Table,
    View,
}

impl RelationKind {
    /// `information_schema.tables.table_type`: `VIEW`, or a table of some kind.
    pub fn from_table_type(table_type: &str) -> Self {
        if table_type.eq_ignore_ascii_case("view") {
            Self::View
        } else {
            Self::Table
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewDdlError {
    /// The name is not one of this preview's schemas; nothing was sent.
    #[error("refused: {0}")]
    Refused(#[from] Refused),
    /// `CREATE SCHEMA` found a schema of that name already there. Whoever made
    /// it, the preview did not (its registry row said so), so it is not the
    /// preview's to write into or drop.
    #[error("{0}")]
    AlreadyExists(String),
    /// Airhouse (or the stand-in) could not be reached or refused the statement.
    #[error("{0}")]
    Backend(String),
}

/// Whether `name` is exactly a schema of `ns`: lowercase, the shape every
/// preview schema has, and under this preview's own prefix. Case matters —
/// the registry stores lowercase names, and a mixed-case name is never one of
/// them.
pub fn well_formed(ns: &PreviewNamespace, name: &str) -> Result<(), Refused> {
    if PREVIEW_SCHEMA_NAME.is_match(name) && ns.owns_schema(name) {
        Ok(())
    } else {
        Err(Refused(format!(
            "{name:?} is not a schema of preview {}",
            ns.key()
        )))
    }
}

/// The fixed statements. Relation names come from `information_schema`, so
/// they are quoted, never spliced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ddl {
    CreateSchema(String),
    DropSchema(String),
    DropRelation(String, Relation),
}

impl Ddl {
    /// The statement to send, or `Refused` when the schema is not `ns`'s.
    ///
    /// `CREATE SCHEMA` is strict (no `IF NOT EXISTS`): a schema that is already
    /// there fails the statement, so the preview never adopts one it did not
    /// make. `DROP SCHEMA` has no `CASCADE`: `previews::drop` sends it only
    /// after dropping every relation the preview recorded, and only when
    /// nothing else is left, so it can never take anything with it.
    pub fn statement(&self, ns: &PreviewNamespace) -> Result<String, Refused> {
        match self {
            Ddl::CreateSchema(schema) => {
                well_formed(ns, schema)?;
                Ok(format!("CREATE SCHEMA {}", quote_ident(schema)))
            }
            Ddl::DropSchema(schema) => {
                well_formed(ns, schema)?;
                Ok(format!("DROP SCHEMA IF EXISTS {}", quote_ident(schema)))
            }
            Ddl::DropRelation(schema, relation) => {
                well_formed(ns, schema)?;
                let verb = match relation.kind {
                    RelationKind::Table => "TABLE",
                    RelationKind::View => "VIEW",
                };
                Ok(format!(
                    "DROP {verb} IF EXISTS {}.{}",
                    quote_ident(schema),
                    quote_ident(&relation.name)
                ))
            }
        }
    }
}

/// `SELECT table_name, table_type FROM information_schema.tables` for one of
/// `ns`'s schemas.
pub fn list_relations_sql(ns: &PreviewNamespace, schema: &str) -> Result<String, Refused> {
    well_formed(ns, schema)?;
    Ok(format!(
        "SELECT table_name, table_type FROM information_schema.tables \
         WHERE table_schema = {} ORDER BY table_name",
        quote_literal(schema)
    ))
}

/// How many schemas of that name exist (`n`): what a failed `CREATE SCHEMA` is
/// checked against, so "already exists" is read from the catalog rather than
/// from an engine's error text. Compared case-insensitively, as DuckDB
/// compares identifiers: a customer's `"PREVIEW_<key>__Marketing"` blocks the
/// preview's lowercase name just the same.
pub fn schema_exists_sql(ns: &PreviewNamespace, schema: &str) -> Result<String, Refused> {
    well_formed(ns, schema)?;
    Ok(format!(
        "SELECT count(*) AS n FROM information_schema.schemata WHERE lower(schema_name) = {}",
        quote_literal(schema)
    ))
}

/// The error a failed `CREATE SCHEMA` becomes: `AlreadyExists` when the
/// catalog now has the schema, else the backend's own error.
pub(crate) fn create_failed(schema: &str, exists: bool, error: String) -> PreviewDdlError {
    if exists {
        PreviewDdlError::AlreadyExists(format!(
            "a schema named {schema:?} already exists and the preview did not create it"
        ))
    } else {
        PreviewDdlError::Backend(error)
    }
}

pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub(crate) fn quote_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Creates a preview schema, strictly: [`PreviewDdlError::AlreadyExists`] when
/// a schema of that name is already there, whoever made it.
/// `previews::registry::ensure_schema` calls it only after the schema's
/// registry row is written, and only when the row says the preview has not
/// created the schema yet.
#[async_trait]
pub trait SchemaCreator: Send + Sync {
    async fn create_schema(&self, schema: &str) -> Result<(), PreviewDdlError>;
}

/// Lists, empties and drops a preview schema. The TTL drop (`previews::drop`)
/// calls it only for registered, well-formed names the preview created, and
/// drops only the relations the preview recorded.
#[async_trait]
pub trait SchemaDropper: Send + Sync {
    async fn list_relations(&self, schema: &str) -> Result<Vec<Relation>, PreviewDdlError>;
    async fn drop_relation(&self, schema: &str, relation: &Relation)
    -> Result<(), PreviewDdlError>;
    async fn drop_schema(&self, schema: &str) -> Result<(), PreviewDdlError>;
}

/// Opens the drop port for one preview. The drop executor holds one, so a
/// test can hand it DuckDB where production hands it Airhouse.
pub trait OpenSchemaDropper: Send + Sync {
    fn open(&self, workspace_id: Uuid, ns: &PreviewNamespace) -> Box<dyn SchemaDropper>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "feat_je_v2_92a1b7";

    fn ns() -> PreviewNamespace {
        PreviewNamespace::from_key(KEY).unwrap()
    }

    #[test]
    fn only_this_previews_lowercase_schemas_are_well_formed() {
        assert!(well_formed(&ns(), &format!("preview_{KEY}__toast_pos")).is_ok());
        for bad in [
            "preview_notes".to_string(),
            "toast_pos".to_string(),
            format!("PREVIEW_{KEY}__toast_pos"),
            format!("preview_{KEY}__"),
            format!("preview_{KEY}__a__b"),
            format!("preview_{KEY}__a-b"),
            format!("preview_{KEY}__x\"; DROP SCHEMA main; --"),
            "preview_feat_je_v3_000000__toast_pos".to_string(),
        ] {
            assert!(well_formed(&ns(), &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn statements_are_fixed_and_quoted() {
        let schema = format!("preview_{KEY}__toast_pos");
        let view = Relation {
            name: "we\"ird".into(),
            kind: RelationKind::View,
        };
        assert_eq!(
            Ddl::DropRelation(schema.clone(), view)
                .statement(&ns())
                .unwrap(),
            format!("DROP VIEW IF EXISTS \"{schema}\".\"we\"\"ird\"")
        );
        assert_eq!(
            Ddl::DropSchema(schema.clone()).statement(&ns()).unwrap(),
            format!("DROP SCHEMA IF EXISTS \"{schema}\"")
        );
        assert!(
            Ddl::DropSchema("toast_pos".into())
                .statement(&ns())
                .is_err()
        );
        assert!(list_relations_sql(&ns(), "main").is_err());
        assert!(schema_exists_sql(&ns(), "preview_notes").is_err());
    }

    /// `IF NOT EXISTS` would let the preview adopt a schema someone else made.
    #[test]
    fn create_schema_is_strict() {
        let schema = format!("preview_{KEY}__toast_pos");
        assert_eq!(
            Ddl::CreateSchema(schema.clone()).statement(&ns()).unwrap(),
            format!("CREATE SCHEMA \"{schema}\"")
        );
    }
}
