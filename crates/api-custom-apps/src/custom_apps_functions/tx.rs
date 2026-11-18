//! Per-invocation registry of open `ctx.tx()` transactions.
//!
//! The isolate cannot hold a database connection, so it holds an **id** and the
//! pinned connection lives here. That indirection is also the security
//! boundary: a script can only name a transaction it was handed, never
//! construct one.
//!
//! Lifetime is one invocation. `ProjectFunctionHost` is built per invocation
//! and dropped when it ends, which drops this registry, which drops every
//! still-open [`SqlTransaction`] — and dropping a Postgres transaction closes
//! its socket, so the server rolls it back. A function that returns (or throws,
//! or is killed on timeout) mid-transaction therefore commits nothing, with no
//! cleanup path of ours needing to have run.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use agentic_connector::SqlTransaction;

/// Max transactions one invocation may hold open at once.
///
/// Each is a real backend connection held for the duration of the author's
/// JavaScript, so this bounds how much of the database's connection budget a
/// single function can occupy. Nested/parallel transactions are a design smell
/// well before four — the ceiling exists to make a runaway loop fail with a
/// sentence instead of exhausting the pool.
const MAX_OPEN: usize = 4;

/// One open transaction, behind its own lock.
///
/// `Option` rather than a bare box so `take` can claim ownership even while a
/// statement is in flight: it waits for that statement's lock, then leaves
/// `None` behind. `Arc::try_unwrap` would fail in exactly that case.
type Slot = std::sync::Arc<tokio::sync::Mutex<Option<Box<dyn SqlTransaction>>>>;

/// Open transactions for one invocation, keyed by the id handed to the isolate.
#[derive(Default)]
pub(super) struct TxRegistry {
    open: tokio::sync::Mutex<HashMap<u64, Slot>>,
    next_id: AtomicU64,
}

impl TxRegistry {
    /// Register a freshly-begun transaction and return its handle id.
    pub(super) async fn insert(&self, tx: Box<dyn SqlTransaction>) -> Result<u64, String> {
        let mut open = self.open.lock().await;
        if open.len() >= MAX_OPEN {
            // `tx` drops here, rolling back the transaction we just opened —
            // rejecting the call must not leak the connection it cost.
            return Err(format!(
                "this invocation already has {MAX_OPEN} transactions open. \
                 Commit or roll one back before opening another."
            ));
        }
        // Ids are per-invocation and monotonic, so a stale handle from an
        // earlier transaction can never collide with a later one.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        open.insert(id, std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx))));
        Ok(id)
    }

    /// Look up the slot for `id`, holding the registry lock only for the lookup.
    ///
    /// **The scope of this lock is load-bearing.** Holding it for a statement's
    /// duration would serialise every transaction in the invocation, and
    /// `MAX_OPEN` allows four — so two transactions contending on the same rows
    /// would deadlock in a way Postgres cannot see: B waits on A's row lock
    /// while holding the registry lock, and A can never take the registry lock
    /// to make progress. The server's deadlock detector never fires, because
    /// A is blocked on a Rust mutex rather than on the database, and nothing
    /// breaks the cycle until the 330s host-op ceiling — with locks held
    /// throughout. Per-slot locks make a genuine cycle a real `40P01` in
    /// milliseconds, which the author can act on.
    async fn slot(&self, id: u64) -> Result<Slot, String> {
        self.open
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(unknown_handle)
    }

    /// Run a row-returning statement on the transaction `id` names.
    pub(super) async fn query(
        &self,
        id: u64,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>, String> {
        let slot = self.slot(id).await?;
        let mut guard = slot.lock().await;
        let tx = guard.as_mut().ok_or_else(unknown_handle)?;
        tx.query(sql, params).await.map_err(|e| e.to_string())
    }

    /// Run a statement for its effect on the transaction `id` names.
    pub(super) async fn exec(
        &self,
        id: u64,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<u64, String> {
        let slot = self.slot(id).await?;
        let mut guard = slot.lock().await;
        let tx = guard.as_mut().ok_or_else(unknown_handle)?;
        tx.exec(sql, params).await.map_err(|e| e.to_string())
    }

    /// Remove the transaction `id` names, handing ownership to the caller so it
    /// can be committed or rolled back (both consume the handle).
    ///
    /// Drops the registry entry first, so a second `take` fails fast rather
    /// than queueing behind the first one's in-flight statement.
    pub(super) async fn take(&self, id: u64) -> Result<Box<dyn SqlTransaction>, String> {
        let slot = self
            .open
            .lock()
            .await
            .remove(&id)
            .ok_or_else(unknown_handle)?;
        let taken = slot.lock().await.take();
        taken.ok_or_else(unknown_handle)
    }

    #[cfg(test)]
    pub(super) async fn len(&self) -> usize {
        self.open.lock().await.len()
    }
}

/// An id with no live transaction behind it. Reached two ways — a handle used
/// after its callback returned, or a fabricated id — and the message covers
/// both without confirming which, so it is not an oracle for probing ids.
pub(super) fn unknown_handle() -> String {
    "no open transaction for this handle — it was already committed or rolled back".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_connector::{ConnectorError, TxParams};

    /// A transaction that records nothing and succeeds at everything — the
    /// registry's bookkeeping is what is under test, not any SQL behaviour.
    /// The real transactional guarantees are covered against a live Postgres in
    /// `agentic-connector`'s `postgres_tx_tests`.
    struct NoopTx;

    #[async_trait::async_trait]
    impl SqlTransaction for NoopTx {
        async fn query(
            &mut self,
            _sql: &str,
            _params: &TxParams,
        ) -> Result<Vec<serde_json::Value>, ConnectorError> {
            Ok(vec![])
        }
        async fn exec(&mut self, _sql: &str, _params: &TxParams) -> Result<u64, ConnectorError> {
            Ok(0)
        }
        async fn commit(self: Box<Self>) -> Result<(), ConnectorError> {
            Ok(())
        }
        async fn rollback(self: Box<Self>) -> Result<(), ConnectorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ids_are_unique_and_never_reused_after_take() {
        let reg = TxRegistry::default();
        let a = reg.insert(Box::new(NoopTx)).await.unwrap();
        let b = reg.insert(Box::new(NoopTx)).await.unwrap();
        assert_ne!(a, b);

        reg.take(a).await.expect("first take succeeds");
        let c = reg.insert(Box::new(NoopTx)).await.unwrap();
        assert_ne!(c, a, "a freed id must not be handed out again");
        assert_ne!(c, b);
    }

    #[tokio::test]
    async fn taking_twice_fails_rather_than_double_committing() {
        let reg = TxRegistry::default();
        let id = reg.insert(Box::new(NoopTx)).await.unwrap();
        reg.take(id).await.expect("first take");
        let err = reg.take(id).await.err().expect("second take must fail");
        assert!(err.contains("no open transaction"), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_handle_is_rejected() {
        let reg = TxRegistry::default();
        let err = reg.take(9999).await.err().expect("fabricated id");
        assert!(err.contains("no open transaction"), "{err}");
    }

    /// The deadlock guard from `slot`'s docs, as a test: a statement in flight
    /// on transaction A must not block a statement on transaction B. With a
    /// registry-wide lock this hangs until the test timeout.
    #[tokio::test]
    async fn a_statement_in_flight_on_one_transaction_does_not_block_another() {
        /// Blocks in `exec` until released, standing in for a row-lock wait.
        struct BlockingTx(std::sync::Arc<tokio::sync::Notify>);

        #[async_trait::async_trait]
        impl SqlTransaction for BlockingTx {
            async fn query(
                &mut self,
                _sql: &str,
                _params: &TxParams,
            ) -> Result<Vec<serde_json::Value>, ConnectorError> {
                Ok(vec![])
            }
            async fn exec(
                &mut self,
                _sql: &str,
                _params: &TxParams,
            ) -> Result<u64, ConnectorError> {
                self.0.notified().await;
                Ok(0)
            }
            async fn commit(self: Box<Self>) -> Result<(), ConnectorError> {
                Ok(())
            }
            async fn rollback(self: Box<Self>) -> Result<(), ConnectorError> {
                Ok(())
            }
        }

        let reg = std::sync::Arc::new(TxRegistry::default());
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        let a = reg
            .insert(Box::new(BlockingTx(gate.clone())))
            .await
            .unwrap();
        let b = reg.insert(Box::new(NoopTx)).await.unwrap();

        let blocked = tokio::spawn({
            let reg = reg.clone();
            async move { reg.exec(a, "UPDATE …", &[]).await }
        });
        // Let A reach its wait before B tries to make progress.
        tokio::task::yield_now().await;

        let b_result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            reg.exec(b, "UPDATE …", &[]),
        )
        .await;
        assert!(
            b_result.is_ok(),
            "B must proceed while A is mid-statement; a registry-wide lock deadlocks here"
        );

        gate.notify_one();
        blocked.await.unwrap().expect("A completes once released");
    }

    #[tokio::test]
    async fn the_open_ceiling_is_enforced_and_does_not_leak_the_rejected_transaction() {
        let reg = TxRegistry::default();
        for _ in 0..MAX_OPEN {
            reg.insert(Box::new(NoopTx)).await.expect("under the cap");
        }
        let err = reg
            .insert(Box::new(NoopTx))
            .await
            .err()
            .expect("over the cap");
        assert!(err.contains("already has"), "{err}");
        assert_eq!(
            reg.len().await,
            MAX_OPEN,
            "the rejected transaction must not be registered"
        );
    }
}
