//! A declared table's change table, `oltp_<writer>.<table>__changes`, planned
//! from every shape its rows were captured under (OLTP change stream design
//! §5.4, "Types" and "Schema changes: the change table only gains columns").
//!
//! - **One column per Postgres attribute,** found by attribute number, typed
//!   for DuckDB ([`DuckType`]). The columns are the union of every shape's
//!   captured ones, in the order they were first seen.
//! - **An added column joins the plan,** null for the rows captured before it.
//!   **A dropped one stays,** null from then on.
//! - **A rename, a type change and a name reused by another attribute** are
//!   [`Unsupported`] for now: each needs a column of its own (design §8, phase
//!   3), and mapping it onto the old one would break the change table's
//!   columns as a contract. The table is held; the writer's others ship.
//!   DuckDB compares identifiers without case, so two attributes whose names
//!   differ only in case reuse one name.

use std::collections::HashMap;

use oxy_oltp::changes::Column;

/// A change-table column's DuckDB type, from the Postgres type it captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuckType {
    SmallInt,
    Integer,
    BigInt,
    Float,
    Double,
    /// `DECIMAL(precision, scale)`.
    Decimal(u8, u8),
    Boolean,
    /// Text: a JSON string's content, any other value's JSON text.
    Varchar,
    /// `json` and `jsonb`, and arrays of them: a `VARCHAR` holding the value's
    /// exact JSON text, a string scalar with its quotes. No json extension is
    /// needed to write it.
    Json,
    Uuid,
    Date,
    Timestamp,
    TimestampTz,
    Time,
    List(Box<DuckType>),
}

impl DuckType {
    /// The DuckDB type for a Postgres type, as `format_type` writes it.
    ///
    /// `json` and `jsonb` are [`DuckType::Json`]. Anything else without a
    /// DuckDB twin is `VARCHAR`, holding the value's text: text and character
    /// types, unconstrained `numeric` and one wider than 38 digits, `interval`,
    /// `bytea`, `time with time zone`, enums and domains. An array is a `LIST`
    /// of its element's type, unless that is `VARCHAR` or JSON: then the whole
    /// array is that type too, as its JSON text.
    pub fn of(pg: &str) -> DuckType {
        if let Some(element) = pg.strip_suffix("[]") {
            return match DuckType::of(element) {
                text @ (DuckType::Varchar | DuckType::Json) => text,
                element => DuckType::List(Box::new(element)),
            };
        }
        match pg {
            "smallint" => DuckType::SmallInt,
            "integer" => DuckType::Integer,
            "bigint" => DuckType::BigInt,
            "real" => DuckType::Float,
            "double precision" => DuckType::Double,
            "boolean" => DuckType::Boolean,
            "uuid" => DuckType::Uuid,
            "date" => DuckType::Date,
            "json" | "jsonb" => DuckType::Json,
            _ => decimal(pg)
                .or_else(|| temporal(pg))
                .unwrap_or(DuckType::Varchar),
        }
    }

    /// The type as DuckDB DDL and casts spell it.
    pub fn sql(&self) -> String {
        match self {
            DuckType::SmallInt => "SMALLINT".into(),
            DuckType::Integer => "INTEGER".into(),
            DuckType::BigInt => "BIGINT".into(),
            DuckType::Float => "FLOAT".into(),
            DuckType::Double => "DOUBLE".into(),
            DuckType::Decimal(precision, scale) => format!("DECIMAL({precision},{scale})"),
            DuckType::Boolean => "BOOLEAN".into(),
            DuckType::Varchar | DuckType::Json => "VARCHAR".into(),
            DuckType::Uuid => "UUID".into(),
            DuckType::Date => "DATE".into(),
            DuckType::Timestamp => "TIMESTAMP".into(),
            DuckType::TimestampTz => "TIMESTAMPTZ".into(),
            DuckType::Time => "TIME".into(),
            DuckType::List(element) => format!("{}[]", element.sql()),
        }
    }
}

/// `numeric(p,s)` (or `numeric(p)`) that DuckDB's `DECIMAL` holds: a precision
/// of 1 to 38 and a scale from 0 to the precision. Postgres also allows a
/// negative scale and one above the precision; those are `VARCHAR`.
fn decimal(pg: &str) -> Option<DuckType> {
    let args = pg.strip_prefix("numeric(")?.strip_suffix(')')?;
    let (precision, scale) = args.split_once(',').unwrap_or((args, "0"));
    let precision: u8 = precision.trim().parse().ok()?;
    let scale: u8 = scale.trim().parse().ok()?;
    ((1..=38).contains(&precision) && scale <= precision)
        .then_some(DuckType::Decimal(precision, scale))
}

/// `timestamp[(n)]` with or without time zone, and `time[(n)]` without one.
fn temporal(pg: &str) -> Option<DuckType> {
    let (base, zoned) = match pg.strip_suffix(" without time zone") {
        Some(base) => (base, false),
        None => (pg.strip_suffix(" with time zone")?, true),
    };
    let base = match base.split_once('(') {
        None => base,
        Some((name, digits)) => {
            let digits = digits.strip_suffix(')')?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            name
        }
    };
    match (base, zoned) {
        ("timestamp", false) => Some(DuckType::Timestamp),
        ("timestamp", true) => Some(DuckType::TimestampTz),
        ("time", false) => Some(DuckType::Time),
        _ => None,
    }
}

/// One recorded shape of a table: the columns its rows were captured with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeVersion {
    pub schema_version: i32,
    /// The captured columns only: the relay removes the excluded ones first.
    pub captured: Vec<Column>,
    /// The primary key's column names, in key order.
    pub primary_key: Vec<String>,
}

/// One attribute column of a change table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeColumn {
    pub name: String,
    pub attnum: i16,
    pub ty: DuckType,
}

/// A change table's attribute columns, and which of them each shape's rows
/// fill. Its own `_` columns are [`oxy_oltp::changes::RESERVED_COLUMNS`],
/// always after these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeTable {
    /// The declared table; its change table is `<table>__changes`.
    pub table: String,
    /// Every attribute column, in first-seen order. None for a table whose
    /// rows are all truncates, which need no shape.
    pub columns: Vec<ChangeColumn>,
    /// Per `schema_version`, each captured name's index in `columns`.
    versions: HashMap<i32, HashMap<String, usize>>,
}

impl ChangeTable {
    /// Plan `table` from every shape of it the batch's rows were captured
    /// under, oldest `schema_version` first. No shapes plans a table of `_`
    /// columns only, for truncate markers.
    pub fn plan(table: &str, versions: &[ShapeVersion]) -> Result<ChangeTable, Unsupported> {
        let mut ordered: Vec<&ShapeVersion> = versions.iter().collect();
        ordered.sort_by_key(|v| v.schema_version);
        let mut planner = Planner::default();
        let mut by_version = HashMap::new();
        for version in ordered {
            let mut names = HashMap::new();
            for column in &version.captured {
                names.insert(column.name.clone(), planner.place(column)?);
            }
            by_version.insert(version.schema_version, names);
        }
        Ok(ChangeTable {
            table: table.to_string(),
            columns: planner.columns,
            versions: by_version,
        })
    }

    /// The column a row captured under `schema_version` fills with its
    /// image's `name`; `None` when that shape did not capture `name`, or is
    /// not one this table was planned from.
    pub fn column(&self, schema_version: i32, name: &str) -> Option<&ChangeColumn> {
        let index = *self.versions.get(&schema_version)?.get(name)?;
        self.columns.get(index)
    }
}

/// The columns planned so far, with the Postgres type each was first seen
/// with: a type change is one of any kind, widening included, even where the
/// DuckDB type would be the same.
#[derive(Default)]
struct Planner {
    columns: Vec<ChangeColumn>,
    pg_types: Vec<String>,
}

impl Planner {
    /// `column`'s index in the plan, added when its attribute is new.
    fn place(&mut self, column: &Column) -> Result<usize, Unsupported> {
        if let Some(index) = self.columns.iter().position(|c| c.attnum == column.attnum) {
            let planned = &self.columns[index];
            if planned.name != column.name {
                return Err(Unsupported::Renamed {
                    attnum: column.attnum,
                    from: planned.name.clone(),
                    to: column.name.clone(),
                });
            }
            if self.pg_types[index] != column.type_name {
                return Err(Unsupported::Retyped {
                    column: column.name.clone(),
                    from: self.pg_types[index].clone(),
                    to: column.type_name.clone(),
                });
            }
            return Ok(index);
        }
        // DuckDB compares identifiers without case: `Total` and `total` are
        // one change-table column, so a new attribute taking either name is
        // a reused name.
        let taken = |c: &ChangeColumn| c.name.eq_ignore_ascii_case(&column.name);
        if self.columns.iter().any(taken) {
            return Err(Unsupported::NameReused {
                column: column.name.clone(),
            });
        }
        self.columns.push(ChangeColumn {
            name: column.name.clone(),
            attnum: column.attnum,
            ty: DuckType::of(&column.type_name),
        });
        self.pg_types.push(column.type_name.clone());
        Ok(self.columns.len() - 1)
    }
}

/// A shape change the change table cannot follow yet (design §5.4 and §8,
/// phase 3). Its table is held, and its rows stay in the log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unsupported {
    #[error("column `{from}` (attribute {attnum}) was renamed `{to}`: a rename is not relayed yet")]
    Renamed {
        attnum: i16,
        from: String,
        to: String,
    },
    #[error("column `{column}` changed type from {from} to {to}: a type change is not relayed yet")]
    Retyped {
        column: String,
        from: String,
        to: String,
    },
    #[error("column name `{column}` now names another attribute: a reused name is not relayed yet")]
    NameReused { column: String },
}

#[cfg(test)]
mod tests;
