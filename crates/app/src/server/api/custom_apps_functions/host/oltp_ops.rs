//! `ctx.oltp.{query,exec}` on the host — read or write the app's OWN per-org
//! OLTP schema (`app_<writer>`) on the managed Postgres tenant, and nothing
//! else.
//!
//! This is the write half `ctx.warehouse` could not give an app: for a
//! `postgres_managed` database `ctx.warehouse` resolves the read-only analyst
//! (org-wide read, `raw_*` included), so a write authenticates and then fails
//! `permission denied`. `ctx.oltp` instead resolves the app's **writer** role,
//! whose DML rights are scoped to the one `app_<writer>` schema — narrower on
//! reads (no `raw_*`) and finally writable.
//!
//! Fail-closed on the `oltp` capability, and run in a one-shot transaction so
//! parameters are bound (never string-concatenated) and a failed statement
//! rolls back rather than leaving a partial write. Outside production a
//! statement goes to the org's staging branch when it has one, where only a
//! statement reaching outside that database is refused; with none, it is sent
//! only when it is a single read (`env_guard`). Either is decided before any
//! tenant connection is opened.

use super::super::env_policy::HostOp;
use super::super::host_call_attrs::QuerySummary;
use super::*;

/// What one statement returned, and the rows it reported.
type StatementOutcome = Result<(serde_json::Value, u64), String>;

impl ProjectFunctionHost {
    /// Dispatch one `ctx.oltp` op — see `FunctionHost::oltp`.
    pub(super) async fn oltp_op(
        &self,
        op: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let writer_name = self.oltp_writer()?;
        let (sql, params) = Self::oltp_statement(payload)?;
        let host_op = HostOp::sub_op("oltp", op).ok_or_else(|| format!("unknown op '{op}'"))?;
        let schema = self.caps.oltp.schema().unwrap_or_default();
        self.admit_statement(host_op, None, &sql, &schema).await?;
        let summary = db_query_summary(&sql);
        let conn = self.oltp_connection(writer_name).await?;
        let mut tx = self.open_statement_tx(host_op, &conn, &summary).await?;
        let outcome = self.run_statement(host_op, &mut *tx, &sql, &params).await;
        match outcome {
            Ok((result, rows)) => {
                with_db_timeout("commit", async move {
                    tx.commit().await.map_err(|e| format!("commit failed: {e}"))
                })
                .await?;
                self.note_statement_write(&conn.schema, summary, rows).await;
                enforce_result_byte_cap(&result)?;
                Ok(result)
            }
            Err(e) => {
                // Roll back explicitly; a rollback failure is moot (the dropped
                // connection rolls back anyway) and must not mask the real error.
                let _ = tx.rollback().await;
                let err = format!("{op}: {e}");
                Err(self
                    .held_oltp_error(host_op, err, &conn.schema, &summary)
                    .await)
            }
        }
    }

    /// The one-shot transaction a statement runs in, named for the audit and,
    /// outside production, `READ ONLY` on a read-only session.
    async fn open_statement_tx(
        &self,
        op: HostOp,
        conn: &oxy_oltp::resolver::WriterConnection,
        summary: &QuerySummary,
    ) -> Result<Box<dyn SqlTransaction>, String> {
        let connector = self.oltp_connector(op, conn)?;
        let mut tx = with_db_timeout("begin", async {
            connector
                .begin_transaction()
                .await
                .map_err(|e| format!("could not open a transaction: {e}"))
        })
        .await?;
        data_audit::record_db_span(summary, Some(&conn.schema), None);
        let (trace_id, _) = Self::trace_context();
        // On error `tx` drops here, which closes the connection and rolls the
        // empty transaction back.
        self.read_only_when_held(op, &mut *tx, &conn.schema).await?;
        self.name_session(&mut *tx, trace_id.as_deref()).await?;
        Ok(tx)
    }

    async fn run_statement(
        &self,
        op: HostOp,
        tx: &mut dyn SqlTransaction,
        sql: &str,
        params: &[serde_json::Value],
    ) -> StatementOutcome {
        let (_, traceparent) = Self::trace_context();
        let tagged = data_audit::commented(sql, &self.identity, traceparent.as_deref());
        match op {
            HostOp::OltpQuery => with_db_timeout("query", async {
                tx.query(&tagged, params).await.map_err(|e| e.to_string())
            })
            .await
            .map(|rows| {
                let n = rows.len() as u64;
                (serde_json::json!({ "rows": rows }), n)
            }),
            HostOp::OltpExec => with_db_timeout("exec", async {
                tx.exec(&tagged, params).await.map_err(|e| e.to_string())
            })
            .await
            .map(|count| (serde_json::json!({ "rowCount": count }), count)),
            other => Err(format!("unknown op '{}'", other.name())),
        }
    }

    /// Buffer a committed write for the invocation's `app.oltp.write` row.
    async fn note_statement_write(&self, schema: &str, summary: QuerySummary, rows: u64) {
        if !data_audit::is_write_verb(&summary.verb) {
            return;
        }
        self.note_write(WriteRecord {
            plane: "oltp",
            namespace: schema.to_string(),
            verb: summary.verb,
            table: summary.table,
            rows: Some(rows),
            statements: 1,
            op: None,
            note: None,
        })
        .await;
    }

    /// A connector for the resolved writer, opening a read-only session when
    /// `op` is held. Verifies the managed peer's certificate (see
    /// `WriterConnection::verify_tls`); the DSN's `sslmode=require` only
    /// encrypts.
    pub(super) fn oltp_connector(
        &self,
        op: HostOp,
        conn: &oxy_oltp::resolver::WriterConnection,
    ) -> Result<Arc<dyn DatabaseConnector>, String> {
        let dsn = self.oltp_dsn(op, &conn.dsn);
        let connector = PostgresConnector::from_dsn(&dsn, conn.verify_tls)
            .map_err(|e| format!("could not build a connection to '{}': {e}", conn.schema))?;
        Ok(Arc::new(connector))
    }
}
