//! The SQL that installs a declared table's capture, commits it, and removes
//! it, each on the connection it needs (OLTP change stream design §5.2,
//! "Install order").
//!
//! The owner owns `app_<writer>`, where the writer only has `USAGE` and
//! `CREATE`, so the functions and the registry rows go in as the owner. The
//! triggers go on, and come off, as the **table's** owner: `CREATE TRIGGER`
//! needs only `TRIGGER` on the table, but `ALTER TABLE … ENABLE ALWAYS
//! TRIGGER` and `DROP TRIGGER` need its ownership. A table the app's
//! migrations created is the writer's, since they run as the writer; one a
//! workspace's `schemas/*.sql` created in the app's schema is the owner's,
//! since they run as the owner and only grant the writer `ALL` on it. The
//! caller reads which role owns each table and runs the trigger SQL on that
//! role's connection. So:
//!
//! 1. **as the owner**, [`owner_install_sql`], in one transaction pinned to
//!    `search_path = pg_catalog, pg_temp`: make sure of the writer's image
//!    role and `oxy_changes.image_<writer>`, which its images are built with
//!    ([`super::image`]), seed `stream` if it has no row,
//!    write the table's `tables` row with `installed = false`, record its
//!    shape, and create the four functions in `app_<writer>`, owned by the
//!    owner, `EXECUTE` revoked from `PUBLIC` and granted to that writer
//!    alone. The writer can neither replace nor drop a function it does not
//!    own in a schema it does not own;
//! 2. **as the table's owner** (the writer, or the owner for a table a
//!    workspace's schemas made), [`writer_install_sql`]: the four
//!    statement-level triggers, `ENABLE ALWAYS` so a
//!    `session_replication_role = replica` session still captures;
//! 3. **as the owner**, [`owner_commit_sql`]: `installed = true`, from which
//!    the DDL guard holds the table.
//!
//! Removing a table runs the same steps backwards: [`owner_uninstall_sql`],
//! [`writer_uninstall_sql`] as the table's owner, [`owner_drop_sql`].
//! `installed` goes false first so the guard lets the triggers go; the shapes
//! stay, for history already shipped.
//!
//! Every identifier here has passed [`validate_name`] (or is derived from
//! one that has, as `app_<writer>` is) and is quoted on top; every name the
//! app chose for a column reaches SQL only as a literal.

use crate::platform::{CHANGES_SHAPES_TABLE, CHANGES_STREAM_TABLE, CHANGES_TABLES_TABLE};
use crate::roles::quote_literal;
use crate::schema::{SchemaError, WriterRef, quote_ident, validate_name};

use super::baseline::{BaselineReason, abandon_baselines_sql, request_baseline_sql};
use super::capture::{self, Op};
use super::image::{OWNER_TRANSACTION_PIN_SQL, image_sql};
use super::shape::CaptureShape;

/// One declared table's capture, as one install writes it.
#[derive(Debug, Clone, Copy)]
pub struct CaptureSpec<'a> {
    /// The app's bare writer name (`bookings`, schema `app_bookings`): what
    /// `tables.writer` and each log row record.
    pub writer: &'a str,
    pub capture: &'a CaptureShape,
    /// What [`super::next_schema_version`] gave for this shape.
    pub schema_version: i32,
}

/// The writer's `tables` rows (`$1`, its bare name), each with its latest
/// `shapes` row (null columns when it has none), for diffing what is
/// installed against what the build declares: `relid`, `schema_name`,
/// `table_name`, `schema_version`, `installed`, then `shape_version`,
/// `columns` (`jsonb`, [`super::Column`]s), `primary_key` and `excluded`
/// (`text[]`).
pub const INSTALLED_SQL: &str = "SELECT t.relid, t.schema_name, t.table_name, t.schema_version, \
       t.installed, s.schema_version AS shape_version, s.columns, s.primary_key, s.excluded \
  FROM oxy_changes.tables t \
  LEFT JOIN LATERAL ( \
    SELECT sh.schema_version, sh.columns, sh.primary_key, sh.excluded \
      FROM oxy_changes.shapes sh \
     WHERE sh.relid = t.relid \
     ORDER BY sh.schema_version DESC \
     LIMIT 1) s ON true \
 WHERE t.writer = $1 \
 ORDER BY t.table_name";

/// Triggers are named this and the operation: `oxy_capture_i`.
const TRIGGER_PREFIX: &str = "oxy_capture_";

/// Step 1 of an install, as the owner, in one transaction (its head is a
/// `SET LOCAL`). `writer_role` is the writer's real role name (qualified on a
/// shared-cluster provider), the one `EXECUTE` is granted to; `image_role` is
/// [`super::image_role_for`] the writer, the role that builds its images.
/// `baseline` is why the table needs a fresh baseline, if it does: its
/// request goes in after the `shapes` row, in the same transaction, so a
/// reshape's request cannot be lost (design §5.2). Both rows it reads are
/// written before it.
pub fn owner_install_sql(
    spec: &CaptureSpec<'_>,
    writer_role: &str,
    image_role: &str,
    baseline: Option<BaselineReason>,
) -> Result<Vec<String>, SchemaError> {
    let target = Target::of(spec)?;
    validate_name(writer_role)?;
    let role = quote_ident(writer_role);
    let shape = &spec.capture.shape;
    let relid = shape.relid;
    let version = spec.schema_version;
    let columns = serde_json::Value::Array(
        spec.capture
            .captured
            .iter()
            .map(|c| serde_json::json!({"attnum": c.attnum, "name": c.name, "type": c.type_name}))
            .collect(),
    );
    // The image function before any capture function that calls it.
    let mut sql = vec![OWNER_TRANSACTION_PIN_SQL.to_string()];
    sql.extend(image_sql(spec.writer, image_role)?);
    sql.extend([
        format!(
            "INSERT INTO {CHANGES_STREAM_TABLE} (generation, capturing) VALUES (1, true) \
             ON CONFLICT (one) DO NOTHING"
        ),
        format!(
            "INSERT INTO {CHANGES_TABLES_TABLE} \
             (relid, schema_name, table_name, writer, schema_version, installed) \
             VALUES ({relid}::oid, {schema}, {table}, {writer}, {version}, false) \
             ON CONFLICT (relid) DO UPDATE SET schema_name = EXCLUDED.schema_name, \
             table_name = EXCLUDED.table_name, writer = EXCLUDED.writer, \
             schema_version = EXCLUDED.schema_version, installed = false",
            schema = quote_literal(&shape.schema),
            table = quote_literal(&shape.table),
            writer = quote_literal(spec.writer),
        ),
        format!(
            "INSERT INTO {CHANGES_SHAPES_TABLE} \
             (relid, schema_version, columns, primary_key, excluded) \
             VALUES ({relid}::oid, {version}, {columns}::jsonb, {key}, {excluded}) \
             ON CONFLICT (relid, schema_version) DO NOTHING",
            columns = quote_literal(&columns.to_string()),
            key = text_array(&shape.primary_key),
            excluded = text_array(&spec.capture.excluded()),
        ),
    ]);
    if let Some(reason) = baseline {
        sql.extend(request_baseline_sql(relid, reason));
    }
    for op in Op::ALL {
        let function = target.function(op);
        sql.push(format!(
            "CREATE OR REPLACE FUNCTION {function} RETURNS trigger LANGUAGE plpgsql \
             SECURITY DEFINER SET search_path = pg_catalog, pg_temp AS {}",
            dollar_quote(&capture::body(spec, op))
        ));
        sql.push(format!("REVOKE ALL ON FUNCTION {function} FROM PUBLIC"));
        sql.push(format!("GRANT EXECUTE ON FUNCTION {function} TO {role}"));
    }
    Ok(sql)
}

/// Step 2 of an install, as the table's owner (the writer, or the owner for a
/// table a workspace's schemas made): enabling a trigger needs ownership, not
/// only `TRIGGER`. One `AFTER … FOR EACH STATEMENT` trigger per operation,
/// each with the transition tables its function reads (Postgres allows them
/// only on a single-event trigger), then each enabled always. `CREATE OR
/// REPLACE`, so a reinstall rewires the triggers in place.
pub fn writer_install_sql(spec: &CaptureSpec<'_>) -> Result<Vec<String>, SchemaError> {
    let target = Target::of(spec)?;
    let table = &target.table;
    let mut sql: Vec<String> = Op::ALL
        .iter()
        .map(|&op| {
            format!(
                "CREATE OR REPLACE TRIGGER {} AFTER {} ON {table} {}FOR EACH STATEMENT \
                 EXECUTE FUNCTION {}",
                trigger_name(op),
                event(op),
                referencing(op),
                target.function(op)
            )
        })
        .collect();
    sql.extend(Op::ALL.iter().map(|&op| {
        format!(
            "ALTER TABLE {table} ENABLE ALWAYS TRIGGER {}",
            trigger_name(op)
        )
    }));
    Ok(sql)
}

/// Step 3 of an install, as the owner: the guard holds the table from here.
pub fn owner_commit_sql(relid: u32, schema_version: i32) -> String {
    format!(
        "UPDATE {CHANGES_TABLES_TABLE} SET installed = true, schema_version = {schema_version} \
         WHERE relid = {relid}::oid"
    )
}

/// Step 1 of a removal, as the owner: the guard lets the table's triggers go.
pub fn owner_uninstall_sql(relid: u32) -> String {
    format!("UPDATE {CHANGES_TABLES_TABLE} SET installed = false WHERE relid = {relid}::oid")
}

/// Step 2 of a removal, as the table's owner, the only role that may drop
/// its triggers. `schema` and `table` name the table where it is now, as the
/// catalog reports it for the oid in its `tables` row.
pub fn writer_uninstall_sql(schema: &str, table: &str) -> Vec<String> {
    let table = format!("{}.{}", quote_ident(schema), quote_ident(table));
    Op::ALL
        .iter()
        .map(|&op| format!("DROP TRIGGER IF EXISTS {} ON {table}", trigger_name(op)))
        .collect()
}

/// Step 3 of a removal, as the owner: the four functions, the table's open
/// baselines abandoned, then the `tables` row. The `shapes` rows stay. No `CASCADE`: a function a trigger still
/// calls means step 2 did not run, and dropping it would leave that trigger
/// failing every write.
pub fn owner_drop_sql(relid: u32, schema: &str) -> Vec<String> {
    let schema = quote_ident(schema);
    let mut sql: Vec<String> = Op::ALL
        .iter()
        .map(|&op| {
            format!(
                "DROP FUNCTION IF EXISTS {}",
                function_ref(&schema, relid, op)
            )
        })
        .collect();
    sql.push(abandon_baselines_sql(relid));
    sql.push(format!(
        "DELETE FROM {CHANGES_TABLES_TABLE} WHERE relid = {relid}::oid"
    ));
    sql
}

/// The names an install's statements are built from, each checked: the
/// writer's schema derives from its validated name, the table's schema must
/// be that one, and the table's name passes [`validate_name`].
struct Target {
    /// `"app_<writer>"`, quoted.
    schema: String,
    /// `"app_<writer>"."<table>"`, quoted.
    table: String,
    relid: u32,
}

impl Target {
    fn of(spec: &CaptureSpec<'_>) -> Result<Self, SchemaError> {
        let shape = &spec.capture.shape;
        let schema = WriterRef::app(spec.writer)?.schema_name();
        if shape.schema != schema {
            return Err(SchemaError::InvalidName(shape.schema.clone()));
        }
        validate_name(&shape.table)?;
        let schema = quote_ident(&schema);
        Ok(Self {
            table: format!("{schema}.{}", quote_ident(&shape.table)),
            schema,
            relid: shape.relid,
        })
    }

    fn function(&self, op: Op) -> String {
        function_ref(&self.schema, self.relid, op)
    }
}

/// `"<schema>"."_oxy_capture_<relid>_<op>"()`, with `schema` already quoted.
fn function_ref(schema: &str, relid: u32, op: Op) -> String {
    format!(
        "{schema}.{}()",
        quote_ident(&capture::function_name(relid, op))
    )
}

fn trigger_name(op: Op) -> String {
    format!("{TRIGGER_PREFIX}{}", op.code())
}

fn event(op: Op) -> &'static str {
    match op {
        Op::Insert => "INSERT",
        Op::Update => "UPDATE",
        Op::Delete => "DELETE",
        Op::Truncate => "TRUNCATE",
    }
}

/// The transition tables each function reads, by the names its body uses.
fn referencing(op: Op) -> &'static str {
    match op {
        Op::Insert => "REFERENCING NEW TABLE AS oxy_new ",
        Op::Update => "REFERENCING OLD TABLE AS oxy_old NEW TABLE AS oxy_new ",
        Op::Delete => "REFERENCING OLD TABLE AS oxy_old ",
        Op::Truncate => "",
    }
}

/// `body` between dollar quotes whose tag it does not contain. A column name
/// is the app's to choose and reaches the body as a literal, so a fixed tag
/// could be closed early by a name that spells it, and what followed would
/// run as the owner.
fn dollar_quote(body: &str) -> String {
    let mut tag = "$oxy$".to_string();
    let mut n = 0u32;
    while body.contains(&tag) {
        n += 1;
        tag = format!("$oxy{n}$");
    }
    format!("{tag}{body}{tag}")
}

/// `ARRAY['a','b']::text[]`, or an empty `text[]`.
fn text_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| quote_literal(s)).collect();
    format!("ARRAY[{}]::text[]", quoted.join(","))
}

#[cfg(test)]
mod tests;
