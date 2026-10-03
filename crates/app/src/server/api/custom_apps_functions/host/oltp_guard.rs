//! How an OLTP statement is admitted outside production — on the org's
//! staging branch, or held on production (`env_guard` has the rest of the
//! mechanism; the decisions are `env_policy`'s).
//!
//! - **On the branch** (previews P4b): every statement runs as asked except
//!   one that reaches outside that database (refused) or whose SQL is decided
//!   only when it runs — `DO`, `CALL`, a function definition (held)
//!   (`env_policy::admit_branch_statement`); either is listed in the held
//!   row. A `ctx.oltp.tx` handle opened there runs its statements and its
//!   commit there.
//! - **In a sandbox's own schema on the branch**: the same, and a statement
//!   that names another schema, or changes how names resolve, is refused too
//!   (`env_policy::admit_sandbox_statement`). While that schema is not ready
//!   every OLTP op is refused before anything connects
//!   ([`ProjectFunctionHost::refuse_unready_sandbox`]).
//! - **Held on production** (no branch): a statement is sent only when it is
//!   one read calling no side-effect function
//!   (`env_policy::admit_oltp_statement`), on a session whose
//!   `default_transaction_read_only` is on, inside a `READ ONLY` transaction —
//!   three layers, so a function the app defined that writes is still refused
//!   by Postgres. Not one that notifies or takes an advisory lock: `READ ONLY`
//!   allows both (see `env_policy::oltp_sql`). Each held entry, and the error,
//!   names how to get a branch.

use super::super::env_policy::{self, Decision, HostOp, Target};
use super::super::host_call_attrs::QuerySummary;
use super::env_guard::{HeldTarget, holds};
use super::*;

/// Appended to an OLTP DSN's `options` outside production: every transaction
/// the session opens is read-only unless a statement lifts it — which the
/// statement classifier never sends.
const READ_ONLY_SESSION: &str = "-cdefault_transaction_read_only%3Don";

impl ProjectFunctionHost {
    /// `Ok` when an OLTP statement may be sent: always in production; on the
    /// org's staging branch, anything that stays inside that database;
    /// otherwise, only one read calling no side-effect function. A held or
    /// refused statement is logged and never reaches the database.
    /// `opened_into` is where a `ctx.oltp.tx` handle's `begin` went (`None`
    /// for a one-shot `ctx.oltp` call, decided by the op).
    pub(super) async fn admit_statement(
        &self,
        op: HostOp,
        opened_into: Option<Target>,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let decided = self.policy.decide_on_handle(op, opened_into);
        if decided == Decision::Isolate(Target::OltpBranch) {
            return self.admit_on_oltp_branch(op, sql, namespace).await;
        }
        self.refuse_unready_sandbox(op, namespace, "STATEMENT")
            .await?;
        if !holds(decided) {
            return Ok(());
        }
        let Err(held) = env_policy::admit_oltp_statement(sql) else {
            return Ok(());
        };
        self.note_held_oltp(op, ("oltp", namespace, &held.verb, &held.table))
            .await;
        Err(self.held_oltp_message(op, &held.why))
    }

    /// A statement bound for the org's staging branch: sent unless it
    /// reaches outside that database (refused) or runs SQL decided only when
    /// it runs (held) — `env_policy::admit_branch_statement`. Either is logged
    /// in the held row and never reaches the branch.
    async fn admit_on_oltp_branch(
        &self,
        op: HostOp,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let (statement, message) = match env_policy::admit_branch_statement(sql) {
            Ok(()) => return self.admit_in_sandbox_schema(op, sql, namespace).await,
            Err(env_policy::NotSent::Refused(statement)) => {
                let message = env_policy::branch_statement_message(
                    op,
                    self.policy.environment(),
                    &statement.why,
                );
                (statement, message)
            }
            Err(env_policy::NotSent::Held(statement)) => {
                let message = env_policy::branch_held_statement_message(op, &statement.why);
                (statement, message)
            }
        };
        self.note_held(op, ("oltp", namespace, &statement.verb, &statement.table))
            .await;
        Err(message)
    }

    /// A statement the branch admits, bound for a **sandbox's** schema there:
    /// sent unless it names another schema or changes how names resolve
    /// (`env_policy::admit_sandbox_statement`). Staging, which runs in the
    /// app's own schema, has no such fence.
    async fn admit_in_sandbox_schema(
        &self,
        op: HostOp,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let env_policy::OltpHome::SandboxSchema(home) = self.policy.oltp_home() else {
            return Ok(());
        };
        let fence = env_policy::SandboxFence::new(home.schema.app_schema(), home.schema.name());
        let Err(statement) = env_policy::admit_sandbox_statement(&fence, sql) else {
            return Ok(());
        };
        self.note_held(op, ("oltp", namespace, &statement.verb, &statement.table))
            .await;
        Err(env_policy::sandbox_statement_message(
            op,
            self.policy.environment(),
            &statement.why,
        ))
    }

    /// `Err` when the policy refuses `op` outright — a sandbox whose own
    /// schema on the org's staging branch is not ready. Noted in the held
    /// row; nothing connects.
    pub(super) async fn refuse_unready_sandbox(
        &self,
        op: HostOp,
        namespace: &str,
        verb: &str,
    ) -> Result<(), String> {
        let Decision::Refuse { fix } = self.policy.decide(op) else {
            return Ok(());
        };
        self.note_held(op, ("oltp", namespace, verb, "")).await;
        Err(env_policy::refused_message(
            op,
            self.policy.environment(),
            fix,
        ))
    }

    /// Where a `ctx.oltp.tx` handle opens into: the staging branch when the
    /// policy isolates `tx.begin_oltp` there, else `None` (as asked).
    pub(super) fn oltp_opened_into(&self) -> Option<Target> {
        match self.policy.decide(HostOp::TxBeginOltp) {
            Decision::Isolate(target) => Some(target),
            _ => None,
        }
    }

    /// Whether this is a staging run whose org has no OLTP staging branch —
    /// the one case a held OLTP call names how to get one.
    fn lacks_a_branch(&self) -> bool {
        !self.policy.is_production() && *self.policy.oltp_home() == env_policy::OltpHome::Production
    }

    /// [`Self::note_held`] for a held OLTP call, carrying why it could not
    /// run on a staging copy instead: the org has no staging branch.
    pub(super) async fn note_held_oltp(&self, op: HostOp, target: HeldTarget<'_>) {
        let note = self.lacks_a_branch().then_some(env_policy::NO_BRANCH_NOTE);
        self.note_held_with(op, target, note).await;
    }

    /// What a held OLTP statement throws: the op's hold, what the statement
    /// was, and — in staging with no branch — how to get one.
    fn held_oltp_message(&self, op: HostOp, why: &str) -> String {
        let held = env_policy::held_statement_message(&self.held_error(op), why);
        self.with_branch_note(held)
    }

    /// `message`, followed by [`env_policy::NO_BRANCH_NOTE`] when this is a
    /// staging run whose org has no OLTP staging branch.
    fn with_branch_note(&self, message: String) -> String {
        if !self.lacks_a_branch() {
            return message;
        }
        format!("{message} This org has {}.", env_policy::NO_BRANCH_NOTE)
    }

    /// The DSN an OLTP connection opens with. Outside production, held, the
    /// session defaults every transaction to read-only.
    pub(super) fn oltp_dsn(&self, op: HostOp, dsn: &str) -> String {
        if self.holds(op) {
            read_only_session(dsn)
        } else {
            dsn.to_string()
        }
    }

    /// Put an OLTP transaction in `READ ONLY` mode when `op` is held, and log
    /// that it was opened so. Postgres then refuses a write the classifier
    /// cannot see — one inside a function the app defined.
    pub(super) async fn read_only_when_held(
        &self,
        op: HostOp,
        tx: &mut dyn SqlTransaction,
        namespace: &str,
    ) -> Result<(), String> {
        if !self.holds(op) {
            return Ok(());
        }
        with_db_timeout("read-only", async {
            tx.exec("SET TRANSACTION READ ONLY", &[])
                .await
                .map(|_| ())
                .map_err(|e| format!("could not open a read-only transaction: {e}"))
        })
        .await?;
        if op == HostOp::TxBeginOltp {
            self.note_held_oltp(op, ("oltp", namespace, "BEGIN READ ONLY", ""))
                .await;
        }
        Ok(())
    }

    /// Turn Postgres's read-only refusal of a held op into the held message,
    /// and log it, so the write reports why it did not happen.
    pub(super) async fn held_oltp_error(
        &self,
        op: HostOp,
        err: String,
        namespace: &str,
        summary: &QuerySummary,
    ) -> String {
        if !self.holds(op) || !env_policy::is_read_only_violation(&err) {
            return err;
        }
        self.note_held_oltp(op, ("oltp", namespace, &summary.verb, &summary.table))
            .await;
        self.with_branch_note(self.held_error(op))
    }
}

/// `dsn` with [`READ_ONLY_SESSION`] added to its `options` parameter — the one
/// that already carries the writer's `search_path` — or as a new one.
fn read_only_session(dsn: &str) -> String {
    let Some((base, query)) = dsn.split_once('?') else {
        return format!("{dsn}?options={READ_ONLY_SESSION}");
    };
    let mut found = false;
    let params: Vec<String> = query
        .split('&')
        .map(|param| match param.strip_prefix("options=") {
            Some(value) => {
                found = true;
                format!("options={value}%20{READ_ONLY_SESSION}")
            }
            None => param.to_string(),
        })
        .collect();
    let mut query = params.join("&");
    if !found {
        // A DSN ending in a bare `?` has no parameter to join onto.
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&format!("options={READ_ONLY_SESSION}"));
    }
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::read_only_session;

    #[test]
    fn the_read_only_default_joins_the_writers_search_path_option() {
        assert_eq!(
            read_only_session(
                "postgresql://w:p@h:5432/db?sslmode=require&options=-csearch_path%3Dapp_x"
            ),
            "postgresql://w:p@h:5432/db?sslmode=require&options=-csearch_path%3Dapp_x\
             %20-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db?sslmode=require"),
            "postgresql://w:p@h/db?sslmode=require&options=-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db"),
            "postgresql://w:p@h/db?options=-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db?"),
            "postgresql://w:p@h/db?options=-cdefault_transaction_read_only%3Don"
        );
    }

    /// The session option survives the DSN parser the connector uses.
    #[test]
    fn the_connector_reads_both_options() {
        let dsn = read_only_session("postgresql://w:p@h/db?options=-csearch_path%3Dapp_x");
        let config: tokio_postgres::Config = dsn.parse().expect("parses");
        assert_eq!(
            config.get_options(),
            Some("-csearch_path=app_x -cdefault_transaction_read_only=on")
        );
    }
}
