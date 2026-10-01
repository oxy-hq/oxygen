//! Resolving the names a preview's SQL uses, and the table functions it may.

use sqlparser::ast::{Ident, ObjectName, ObjectNamePart, TableFactor};

use super::{PreviewNamespace, Refused, RewriteOptions};

/// Where an unqualified table, or one named `<catalog>.t`, lives.
const DEFAULT_SCHEMA: &str = "main";

/// Table functions that compute rows or read files, and touch no table. The
/// rest (`postgres_*`, `ducklake_*`, `query`, `query_table`, …) can reach
/// DuckLake's metadata or another database and so write around every rule.
const PREVIEW_TABLE_FUNCTIONS: [&str; 9] = [
    "range",
    "generate_series",
    "unnest",
    "read_parquet",
    "parquet_scan",
    "read_csv",
    "read_csv_auto",
    "read_json",
    "read_json_auto",
];

/// A table name, resolved the way DuckDB binds it: `t` has no schema; `S.t`
/// is schema `S`; `<catalog>.t` in the workspace's catalog is
/// `<catalog>.main.t`, as DuckDB reads a catalog name in that place; and
/// `<catalog>.S.t` must be in the workspace's catalog.
pub(super) struct Name {
    /// The catalog as written, when the name had one (it is the workspace's).
    catalog: Option<Ident>,
    /// The schema, lowercase; `None` when unqualified.
    schema: Option<String>,
    /// The table as written, so the preview's copy keeps its case.
    table: Ident,
}

impl Name {
    pub fn resolve(name: &ObjectName, verb: &str, opts: &RewriteOptions) -> Result<Self, Refused> {
        let parts = plain_parts(name, verb)?;
        let lower = |ident: &Ident| ident.value.to_ascii_lowercase();
        let (catalog, schema, table) = match parts.as_slice() {
            [table] => (None, None, *table),
            [first, table] if is_workspace_catalog(first, opts) => (
                Some((*first).clone()),
                Some(DEFAULT_SCHEMA.to_string()),
                *table,
            ),
            [schema, table] => (None, Some(lower(schema)), *table),
            [catalog, schema, table] => {
                check_catalog(catalog, opts)?;
                (Some((*catalog).clone()), Some(lower(schema)), *table)
            }
            _ => return Err(too_many_parts(verb, name)),
        };
        Ok(Self {
            catalog,
            schema,
            table: table.clone(),
        })
    }

    /// The bare table name when there is no schema (so it may be a CTE).
    pub fn unqualified(&self) -> Option<&str> {
        self.schema.is_none().then_some(self.table.value.as_str())
    }

    /// Lowercase live `(schema, table)`; an unqualified name is in `main`.
    pub fn live(&self) -> (String, String) {
        (
            self.schema.clone().unwrap_or_else(|| DEFAULT_SCHEMA.into()),
            self.table.value.to_ascii_lowercase(),
        )
    }

    /// `[<catalog>.]"<schema>"."t"`.
    pub fn in_schema(&self, schema: &str) -> ObjectName {
        let mut parts = Vec::with_capacity(3);
        parts.extend(self.catalog.clone());
        parts.push(Ident::with_quote('"', schema));
        parts.push(Ident::with_quote('"', self.table.value.clone()));
        ObjectName::from(parts)
    }
}

/// A write target, resolved: the live table it names and its stand-in.
pub(super) struct Target {
    name: Name,
    /// Lowercase live `(schema, table)`: the shadow map's key.
    pub live: (String, String),
    pub preview_schema: String,
    /// The name as written, for statements that read the live table.
    pub live_name: ObjectName,
}

impl Target {
    /// `[<catalog>.]"preview_<key>__S"."t"`.
    pub fn preview_name(&self) -> ObjectName {
        self.name.in_schema(&self.preview_schema)
    }

    /// The live key of `new_table` in this target's schema (a rename).
    pub fn sibling(&self, new_table: &Ident) -> (String, String) {
        (self.live.0.clone(), new_table.value.to_ascii_lowercase())
    }
}

/// Resolve a write target: `S.t`, or `<catalog>.S.t` / `<catalog>.t` in the
/// workspace's own catalog. An unqualified name is refused.
pub(super) fn target(
    name: &ObjectName,
    verb: &str,
    ns: &PreviewNamespace,
    opts: &RewriteOptions,
) -> Result<Target, Refused> {
    let resolved = Name::resolve(name, verb, opts)?;
    if let Some(table) = resolved.unqualified() {
        return Err(Refused(format!(
            "{verb} {name}: name it <schema>.{table} — in a preview an unqualified write is \
             refused, because what it resolves to depends on the session"
        )));
    }
    let live = resolved.live();
    let preview_schema = ns
        .schema_for(&live.0)
        .map_err(|Refused(why)| Refused(format!("{verb} {name}: {why}")))?;
    Ok(Target {
        name: resolved,
        live,
        preview_schema,
        live_name: name.clone(),
    })
}

/// A schema name in `CREATE SCHEMA`: `S` or `<catalog>.S`. Returns the live
/// schema, lowercase.
pub(super) fn schema(name: &ObjectName, opts: &RewriteOptions) -> Result<String, Refused> {
    match plain_parts(name, "CREATE SCHEMA")?.as_slice() {
        [schema] => Ok(schema.value.to_ascii_lowercase()),
        [catalog, schema] => {
            check_catalog(catalog, opts)?;
            Ok(schema.value.to_ascii_lowercase())
        }
        _ => Err(too_many_parts("CREATE SCHEMA", name)),
    }
}

/// The name's identifiers, refusing a computed part. (Whether a *read* name
/// looks like a file is the caller's check: `sql_parse::check_relation`.)
pub(super) fn plain_parts<'n>(name: &'n ObjectName, verb: &str) -> Result<Vec<&'n Ident>, Refused> {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => Ok(ident),
            _ => Err(Refused(format!(
                "{verb} {name}: a computed name is not allowed in a preview"
            ))),
        })
        .collect()
}

fn is_workspace_catalog(ident: &Ident, opts: &RewriteOptions) -> bool {
    opts.catalog
        .as_ref()
        .is_some_and(|own| own.eq_ignore_ascii_case(&ident.value))
}

/// A three-part name must be in the workspace's own catalog.
fn check_catalog(catalog: &Ident, opts: &RewriteOptions) -> Result<(), Refused> {
    if is_workspace_catalog(catalog, opts) {
        return Ok(());
    }
    Err(Refused(format!(
        "catalog {catalog} is not this workspace's Airhouse catalog; a preview reads and writes \
         only there"
    )))
}

fn too_many_parts(verb: &str, name: &ObjectName) -> Refused {
    Refused(format!(
        "{verb} {name}: a table's name has at most three parts, catalog.schema.table"
    ))
}

/// A FROM-clause item other than a plain table: subqueries, joins and the
/// pure or file-reading table functions pass; anything else is refused.
pub(super) fn table_factor(factor: &TableFactor) -> Result<(), Refused> {
    match factor {
        TableFactor::Table { args: None, .. }
        | TableFactor::Derived { .. }
        | TableFactor::NestedJoin { .. }
        | TableFactor::UNNEST { .. }
        | TableFactor::Pivot { .. }
        | TableFactor::Unpivot { .. }
        | TableFactor::MatchRecognize { .. } => Ok(()),
        TableFactor::Table { name, .. } | TableFactor::Function { name, .. } => {
            if is_preview_table_function(name) {
                Ok(())
            } else {
                Err(Refused(format!(
                    "table function {name} is not allowed in a preview: it can reach another \
                     database or DuckLake's metadata and write around the preview (only {} are)",
                    PREVIEW_TABLE_FUNCTIONS.join(", ")
                )))
            }
        }
        _ => Err(Refused(
            "this FROM-clause construct is not allowed in a preview".into(),
        )),
    }
}

fn is_preview_table_function(name: &ObjectName) -> bool {
    matches!(
        name.0.as_slice(),
        [ObjectNamePart::Identifier(ident)]
            if PREVIEW_TABLE_FUNCTIONS.iter().any(|f| ident.value.eq_ignore_ascii_case(f))
    )
}
