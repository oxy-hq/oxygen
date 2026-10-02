//! One sandbox, one piece of background work at a time.
//!
//! A teardown removes a sandbox's homes; a migrations task creates one of
//! them. Two teardowns side by side, or a teardown beside an apply, read the
//! sandbox's row once and then act on whatever the stores hold by the time
//! each step runs — which is how a late run could remove the homes of a
//! sandbox created again under the same name. So both tasks hold this lock
//! for their whole run and read the row **under** it: whoever comes second
//! sees what the first left, never a state it is still changing.
//!
//! A Postgres transaction-scoped advisory lock on a pooled connection of its
//! own, as an Airhouse apply holds (`custom_apps_migrations::airhouse`):
//! ending the transaction releases it, and so does a dropped connection, so a
//! worker that dies mid-run never leaves a sandbox locked.

use std::time::Duration;

use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

/// Tells this lock apart from every other advisory lock in the database.
const SANDBOX_LOCK_SALT: i64 = 0x7361_6e64_626f_7865; // "sandboxe"

/// The advisory-lock key of one sandbox: FNV-1a over the app's id and the
/// sandbox's name.
pub fn lock_key(app_id: Uuid, environment: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in app_id.as_bytes().iter().chain(environment.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash as i64) ^ SANDBOX_LOCK_SALT
}

/// The lock of one sandbox, held until [`release`](Self::release) or drop.
pub struct SandboxLock {
    txn: sqlx::Transaction<'static, sqlx::Postgres>,
}

impl SandboxLock {
    /// Take the lock if nobody holds it: `None` when someone does.
    pub async fn try_acquire(
        db: &DatabaseConnection,
        app_id: Uuid,
        environment: &str,
    ) -> Result<Option<Self>, DbErr> {
        let failed = |step: &str, e: sqlx::Error| DbErr::Custom(format!("{step}: {e}"));
        let mut txn = db
            .get_postgres_connection_pool()
            .begin()
            .await
            .map_err(|e| failed("open the sandbox lock", e))?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(lock_key(app_id, environment))
            .fetch_one(&mut *txn)
            .await
            .map_err(|e| failed("take the sandbox lock", e))?;
        Ok(got.then_some(Self { txn }))
    }

    /// [`try_acquire`](Self::try_acquire), again after each of `waits` while
    /// someone else holds it. `None` when it is still held after the last.
    pub async fn acquire(
        db: &DatabaseConnection,
        app_id: Uuid,
        environment: &str,
        waits: &[Duration],
    ) -> Result<Option<Self>, DbErr> {
        for wait in waits {
            if let Some(lock) = Self::try_acquire(db, app_id, environment).await? {
                return Ok(Some(lock));
            }
            tokio::time::sleep(*wait).await;
        }
        Self::try_acquire(db, app_id, environment).await
    }

    /// Let the lock go. A failed rollback is moot: the connection closing
    /// ends the transaction too.
    pub async fn release(self) {
        let _ = self.txn.rollback().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One key per (app, sandbox): another sandbox of the app, the same name
    /// in another app, and a name this one is a prefix of all lock apart.
    #[test]
    fn each_sandbox_has_a_key_of_its_own() {
        let (app, other) = (Uuid::from_u128(7), Uuid::from_u128(8));
        let key = lock_key(app, "dev-a1");
        assert_eq!(key, lock_key(app, "dev-a1"), "stable");
        for (id, name) in [(app, "dev-b2"), (other, "dev-a1"), (app, "dev-a1-b")] {
            assert_ne!(key, lock_key(id, name), "{id} {name}");
        }
    }
}
