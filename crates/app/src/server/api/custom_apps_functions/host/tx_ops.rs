//! `ctx.tx` and `ctx.oltp.tx` on the host — six verbs over one op, dispatched
//! onto the per-invocation `TxRegistry`.
//!
//! The two opening verbs are the only ones that touch authorization: `begin`
//! runs the same fail-closed `destinations` check as `ctx.warehouse` before a
//! connector exists, and `begin_oltp` the `oltp` gate on the app's own writer.
//! The other four take an id that `begin` handed out, and the registry rejects
//! any id it did not issue — so a script cannot reach a database by guessing a
//! number.
//!
//! Outside production (`env_guard`, `warehouse_home`): a warehouse `begin`
//! opens on the database `nonProduction.destinations` maps the named one to,
//! and every statement and the commit on that handle run there — a statement
//! that reaches past the mapped database is held unsent
//! (`env_policy::destination_sql`); a `begin` on an unmapped database is held.
//! An OLTP handle opens on the org's staging branch when it has one, and every
//! statement and the commit on that handle run there (P4b); with none it
//! opens `READ ONLY` on a read-only session, each statement on it is sent
//! only when it is a single read, and `commit` rolls back.

use super::super::env_policy::destination_sql::DestinationFence;
use super::super::env_policy::{HostOp, Target};
use super::super::host_call_attrs::QuerySummary;
use super::*;

impl ProjectFunctionHost {
    /// Dispatch one `ctx.tx` verb — see `FunctionHost::tx`.
    pub(super) async fn tx_op(
        &self,
        op: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let host_op = HostOp::sub_op("tx", op).ok_or_else(|| format!("unknown op '{op}'"))?;
        match host_op {
            HostOp::TxBegin => self.begin_warehouse_tx(payload).await,
            HostOp::TxBeginOltp => self.begin_oltp_tx().await,
            HostOp::TxQuery => self.tx_query(payload).await,
            HostOp::TxExec => self.tx_exec(payload).await,
            HostOp::TxCommit | HostOp::TxRollback => self.tx_finish(host_op, payload).await,
            _ => Err(format!("unknown op '{op}'")),
        }
    }

    /// Open `ctx.tx(database, fn)`: the same fail-closed destination check as
    /// `ctx.warehouse`, before a connector — and so a credential — exists.
    async fn begin_warehouse_tx(
        &self,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let database = payload
            .get("database")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "`database` is required".to_string())?;
        // Checked as a write even when the callback only reads: `begin` cannot
        // know what the statements will be.
        self.check_write_destination(database, WriteSurface::Transaction)?;
        // After the destination gate, never instead of it (design §4.2): the
        // mapped database outside production, which passes the gate too.
        let (database, mapped) = self
            .write_database(
                HostOp::TxBegin,
                database,
                WriteSurface::Transaction,
                ("warehouse", database, "BEGIN", ""),
            )
            .await?;
        let database = database.as_str();
        let connector = self.connect(database).await?;
        let mut tx = with_db_timeout("begin", async {
            connector
                .begin_transaction()
                .await
                .map_err(|e| format!("could not open a transaction on '{database}': {e}"))
        })
        .await?;
        data_audit::record_db_namespace(database);
        if connector.dialect() == SqlDialect::Postgres {
            let (trace_id, _) = Self::trace_context();
            // On error `tx` drops here, which closes the connection and rolls
            // the empty transaction back.
            self.name_session(&mut *tx, trace_id.as_deref()).await?;
        }
        let fence = mapped.map(|m| m.fence(connector.dialect()));
        self.register_tx(
            tx,
            data_audit::plane_for_dialect(connector.dialect()),
            database,
            fence.as_ref().map(|_| Target::MappedDestination),
            fence,
        )
        .await
    }

    /// Open `ctx.oltp.tx(fn)` on the app's own writer. No database crosses the
    /// boundary, so there is no allowlist to check — only the `oltp` gate. The
    /// connection is the policy's OLTP home: production's, or the org's
    /// staging branch, which the handle then records it opened into.
    async fn begin_oltp_tx(&self) -> Result<serde_json::Value, String> {
        let writer_name = self.oltp_writer()?;
        let schema = self.caps.oltp.schema().unwrap_or_default();
        self.refuse_unready_sandbox(HostOp::TxBeginOltp, &schema, "BEGIN")
            .await?;
        let conn = self.oltp_connection(writer_name).await?;
        let connector = self.oltp_connector(HostOp::TxBeginOltp, &conn)?;
        let mut tx = with_db_timeout("begin", async {
            connector
                .begin_transaction()
                .await
                .map_err(|e| format!("could not open a transaction: {e}"))
        })
        .await?;
        data_audit::record_db_namespace(&conn.schema);
        let (trace_id, _) = Self::trace_context();
        // On error `tx` drops here, which closes the connection and rolls the
        // empty transaction back.
        self.read_only_when_held(HostOp::TxBeginOltp, &mut *tx, &conn.schema)
            .await?;
        self.name_session(&mut *tx, trace_id.as_deref()).await?;
        let opened_into = self.oltp_opened_into();
        self.register_tx(tx, "oltp", &conn.schema, opened_into, None)
            .await
    }

    /// Hand an open transaction to the registry and start its audit record.
    /// `opened_into` is the isolated home its `begin` opened into (`None` as
    /// asked); a handle on a mapped destination carries its statement `fence`.
    async fn register_tx(
        &self,
        tx: Box<dyn SqlTransaction>,
        plane: &'static str,
        namespace: &str,
        opened_into: Option<Target>,
        fence: Option<DestinationFence>,
    ) -> Result<serde_json::Value, String> {
        let id = self.transactions.insert(tx).await?;
        self.tx_writes.lock().await.insert(
            id,
            TxAudit {
                plane,
                database: namespace.to_string(),
                writes: Vec::new(),
                opened_into,
                fence,
            },
        );
        Ok(serde_json::json!({ "id": id }))
    }

    /// Check one statement on handle `id` against the environment policy,
    /// span it, and tag it. `(id, summary, tagged SQL, params)`.
    async fn tx_prepare(
        &self,
        op: HostOp,
        payload: &serde_json::Value,
    ) -> Result<(u64, QuerySummary, String, Vec<serde_json::Value>), String> {
        let (id, sql, params) = Self::tx_statement(payload)?;
        let database = self.tx_database(id).await;
        let opened_into = self.tx_opened_into(id).await;
        let namespace = database.as_deref().unwrap_or_default();
        self.admit_statement(op, opened_into, &sql, namespace)
            .await?;
        if let Some(fence) = self.tx_fence(id).await {
            self.admit_on_mapped(op, &fence, &sql, namespace).await?;
        }
        let summary = db_query_summary(&sql);
        data_audit::record_db_span(&summary, database.as_deref(), None);
        let (_, traceparent) = Self::trace_context();
        let tagged = data_audit::commented(&sql, &self.identity, traceparent.as_deref());
        Ok((id, summary, tagged, params))
    }

    async fn tx_query(&self, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
        let (id, summary, tagged, params) = self.tx_prepare(HostOp::TxQuery, payload).await?;
        let rows =
            match with_db_timeout("query", self.transactions.query(id, &tagged, &params)).await {
                Ok(rows) => rows,
                Err(e) => return Err(self.held_tx_error(HostOp::TxQuery, id, e, &summary).await),
            };
        self.note_tx_write(id, summary, Some(rows.len() as u64))
            .await;
        let result = serde_json::json!({ "rows": rows });
        enforce_result_byte_cap(&result)?;
        Ok(result)
    }

    async fn tx_exec(&self, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
        let (id, summary, tagged, params) = self.tx_prepare(HostOp::TxExec, payload).await?;
        let count =
            match with_db_timeout("exec", self.transactions.exec(id, &tagged, &params)).await {
                Ok(count) => count,
                Err(e) => return Err(self.held_tx_error(HostOp::TxExec, id, e, &summary).await),
            };
        self.note_tx_write(id, summary, Some(count)).await;
        Ok(serde_json::json!({ "rowCount": count }))
    }

    /// `commit` or `rollback`. A held commit rolls back, and is logged.
    async fn tx_finish(
        &self,
        op: HostOp,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = payload
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| "`id` is required".to_string())?;
        // Bounded like the other ops: `take` waits on the slot lock, so a
        // commit racing a statement on the same handle (an author bug) would
        // otherwise wait unbounded on that statement.
        let tx = with_db_timeout("take", self.transactions.take(id)).await?;
        let committing = op == HostOp::TxCommit && !self.held_commit(id).await;
        let label = if committing { "commit" } else { "rollback" };
        let verb = if op == HostOp::TxCommit {
            "commit"
        } else {
            "rollback"
        };
        // Taken from the registry first, so a timeout here still drops the
        // handle — which closes the connection, which rolls back.
        with_db_timeout(label, async move {
            let outcome = if committing {
                tx.commit().await
            } else {
                tx.rollback().await
            };
            outcome.map_err(|e| format!("{verb} failed: {e}"))
        })
        .await?;
        // A rollback leaves no trace: nothing was written.
        let audit = self.tx_writes.lock().await.remove(&id);
        if committing && let Some(audit) = audit {
            let (trace_id, _) = Self::trace_context();
            self.record_writes(
                data_audit::ACTION_TX_COMMIT,
                audit.writes,
                trace_id.as_deref(),
            )
            .await;
        }
        Ok(serde_json::json!({ "ok": true }))
    }

    /// Whether a commit of handle `id` is held here; if so it is logged, and
    /// the caller rolls back instead.
    async fn held_commit(&self, id: u64) -> bool {
        let opened_into = self.tx_opened_into(id).await;
        if !self.holds_on_handle(HostOp::TxCommit, opened_into) {
            return false;
        }
        let (plane, namespace) = self
            .tx_writes
            .lock()
            .await
            .get(&id)
            .map(|a| (a.plane, a.database.clone()))
            .unwrap_or(("oltp", String::new()));
        let target = (plane, namespace.as_str(), "COMMIT", "");
        if plane == "oltp" {
            self.note_held_oltp(HostOp::TxCommit, target).await;
        } else {
            self.note_held(HostOp::TxCommit, target).await;
        }
        true
    }

    /// Postgres's read-only refusal of a statement on handle `id`, as the held
    /// message, logged under the handle's schema.
    async fn held_tx_error(
        &self,
        op: HostOp,
        id: u64,
        err: String,
        summary: &QuerySummary,
    ) -> String {
        let opened_into = self.tx_opened_into(id).await;
        if !self.holds_on_handle(op, opened_into) {
            return err;
        }
        let namespace = self.tx_database(id).await.unwrap_or_default();
        self.held_oltp_error(op, err, &namespace, summary).await
    }

    /// The statement fence of handle `id`, when it is on a mapped destination.
    async fn tx_fence(&self, id: u64) -> Option<DestinationFence> {
        self.tx_writes
            .lock()
            .await
            .get(&id)
            .and_then(|a| a.fence.clone())
    }

    /// The isolated home handle `id` was opened into; `None` for one opened
    /// as asked, or an id `begin` never handed out.
    async fn tx_opened_into(&self, id: u64) -> Option<Target> {
        self.tx_writes
            .lock()
            .await
            .get(&id)
            .and_then(|a| a.opened_into)
    }
}
