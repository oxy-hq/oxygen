//! `ctx.airhouse` on the host: an app's own facts in its workspace's Airhouse.
//!
//! Split out of `host.rs` by surface. What SQL may be sent is decided by
//! `airhouse::sql_rules`; this module owns the connection, the capability gate
//! and the audit record.

use airhouse::sql_rules::{self, Access};

use super::*;

impl ProjectFunctionHost {
    /// Dispatch one `ctx.airhouse` op — see `FunctionHost::airhouse`.
    pub(super) async fn airhouse_op(
        &self,
        op: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let schema = self.airhouse_schema()?;
        match op {
            "query" => self.airhouse_query(&schema, payload).await,
            "exec" => {
                let sql = checked_statement(required_str(payload, "sql")?, &schema, Access::Write)?;
                self.airhouse_write(&schema, &sql, None, None).await?;
                Ok(serde_json::json!({ "ok": true }))
            }
            "append" => {
                let (sql, table, rows) = build_append_sql(&schema, payload)?;
                // Host-built, but sent the way every other statement is: only
                // what `sql_rules` checked and re-rendered leaves this module.
                let sql = checked_statement(&sql, &schema, Access::Write)?;
                self.airhouse_write(&schema, &sql, Some(table), Some(rows))
                    .await?;
                Ok(serde_json::json!({ "rowCount": rows }))
            }
            other => Err(format!("unknown op '{other}'")),
        }
    }

    /// The app's own Airhouse schema, or why `ctx.airhouse` is closed.
    fn airhouse_schema(&self) -> Result<String, String> {
        match &self.caps.airhouse {
            WriterCapability::Disabled => Err(
                "AirhouseCapabilityMissing: this function has not declared the `airhouse` \
                 capability (add \"airhouse\": { \"enabled\": true } to its oxy-app.json entry). \
                 The schema it writes is derived from the app's own slug — the manifest only \
                 enables access."
                    .to_string(),
            ),
            WriterCapability::SlugNotDerivable { slug } => {
                Err(slug_cannot_back_a_schema("Airhouse", slug))
            }
            enabled => enabled
                .schema()
                .ok_or_else(|| "this app's writer cannot name an Airhouse schema".to_string()),
        }
    }

    async fn airhouse_connector(&self, schema: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        let mut cached = self.airhouse_conn.lock().await;
        if let Some(connector) = cached.as_ref() {
            return Ok(connector.clone());
        }
        let connector = with_db_timeout("connect", async {
            self.proj_ctx
                .build_app_airhouse_connector(&self.identity.app_slug, schema)
                .await
                .map_err(|e| format!("could not connect to this app's Airhouse schema: {e}"))
        })
        .await?;
        *cached = Some(connector.clone());
        Ok(connector)
    }

    async fn airhouse_query(
        &self,
        schema: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let sql = checked_statement(required_str(payload, "sql")?, schema, Access::Read)?;
        data_audit::record_db_span(&db_query_summary(&sql), Some(schema), None);
        let connector = self.airhouse_connector(schema).await?;
        let (rows, truncated) =
            query_with_truncation(&self.query_exec, connector, &sql, FUNCTION_MAX_ROWS).await?;
        let result = serde_json::json!({ "rows": rows, "truncated": truncated });
        enforce_result_byte_cap(&result)?;
        Ok(result)
    }

    /// Send one checked write and audit it as the app's own Airhouse write.
    async fn airhouse_write(
        &self,
        schema: &str,
        sql: &str,
        table: Option<&str>,
        rows: Option<u64>,
    ) -> Result<(), String> {
        let summary = db_query_summary(sql);
        data_audit::record_db_span(&summary, Some(schema), table);
        let connector = self.airhouse_connector(schema).await?;
        let (_, traceparent) = Self::trace_context();
        let tag = data_audit::tag(&self.identity, traceparent.as_deref());
        with_db_timeout("airhouse write", async {
            connector
                .execute_statement_tagged(sql, &tag)
                .await
                .map_err(|e| format!("write failed: {e}"))
        })
        .await?;
        if data_audit::is_write_verb(&summary.verb) {
            let table = if summary.table.is_empty() {
                table.unwrap_or_default().to_string()
            } else {
                summary.table
            };
            self.note_write(WriteRecord {
                plane: data_audit::PLANE_APP_AIRHOUSE,
                namespace: schema.to_string(),
                verb: summary.verb,
                table,
                rows,
                statements: 1,
            })
            .await;
        }
        Ok(())
    }
}

/// `ctx.airhouse.append(table, rows)` → `INSERT INTO "<schema>"."<table>" …`.
///
/// The table must be a bare name: the schema is the host's to add, and a name
/// that carried one would be a way to ask for someone else's.
fn build_append_sql<'a>(
    schema: &str,
    payload: &'a serde_json::Value,
) -> Result<(String, &'a str, u64), String> {
    let table = payload
        .get("table")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "`table` is required".to_string())?;
    if !is_plain_identifier(table) {
        return Err(format!(
            "`table` must be a bare lowercase table name such as \"visits\" — append writes into \
             {schema} itself, so pass no schema (got {table:?})"
        ));
    }
    let (_, values) = columns_and_values(payload)?;
    let rows = payload["rows"].as_array().map_or(0, |r| r.len() as u64);
    let sql = format!(
        "INSERT INTO {}.{} {values}",
        quote_ident(schema),
        quote_ident(table)
    );
    Ok((sql, table, rows))
}

/// `^[a-z_][a-z0-9_]{0,62}$` — a name that needs no quoting to mean itself.
fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && name.len() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Check one app statement against its Airhouse schema and return it as parsed
/// and re-rendered — the text to send. Sending the author's original instead
/// would let DuckDB's lexer find a second statement the check never saw.
fn checked_statement(sql: &str, schema: &str, access: Access) -> Result<String, String> {
    let mut statements = sql_rules::check(sql, schema, access).map_err(|e| e.to_string())?;
    match statements.len() {
        1 => Ok(statements.remove(0)),
        n => Err(format!(
            "{n} statements in one call; send one statement per call"
        )),
    }
}

fn required_str<'a>(payload: &'a serde_json::Value, field: &str) -> Result<&'a str, String> {
    payload
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("`{field}` is required"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_qualifies_the_table_with_the_apps_schema_and_passes_the_rules() {
        let payload = serde_json::json!({ "table": "visits", "rows": [{ "visit_id": "v1" }] });
        let (sql, table, rows) = build_append_sql("app_store_ops", &payload).expect("sql");
        assert_eq!(
            sql,
            r#"INSERT INTO "app_store_ops"."visits" ("visit_id") VALUES ('v1')"#
        );
        assert_eq!((table, rows), ("visits", 1));
        sql_rules::check(&sql, "app_store_ops", Access::Write).expect("the host's own SQL passes");
    }

    #[test]
    fn append_refuses_a_table_name_carrying_a_schema() {
        for table in ["toast_pos.orders", "\"visits\"", "Visits", ""] {
            let payload = serde_json::json!({ "table": table, "rows": [{ "a": 1 }] });
            assert!(
                build_append_sql("app_store_ops", &payload).is_err(),
                "{table:?}"
            );
        }
    }

    #[test]
    fn the_airhouse_schema_is_derived_like_the_oltp_one() {
        assert_eq!(
            WriterCapability::resolve(true, "store-ops")
                .schema()
                .as_deref(),
            Some("app_store_ops")
        );
        assert_eq!(WriterCapability::resolve(false, "store-ops").schema(), None);
    }
}
