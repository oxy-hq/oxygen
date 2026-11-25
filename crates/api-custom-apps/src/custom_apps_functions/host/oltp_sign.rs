//! The host signs who made each change in a production `ctx.oltp` transaction
//! (OLTP change stream design §5.2, "Who made it, signed";
//! `oltp_changes::provenance`).
//!
//! Right after a `ctx.oltp` transaction opens and its session is named
//! (`open_statement_tx`, for a one-shot statement or a publish;
//! `begin_oltp_tx`, for a `ctx.oltp.tx` handle), and before any statement of
//! the app's runs, the host reads the transaction id and sets the three
//! settings the capture trigger copies into each log row: the run's actor,
//! its invocation, and the MAC over both, the id, the environment and the
//! writer's schema. Two round trips, on the transaction's own connection; the
//! key never leaves this process.
//!
//! Only when the build declares a table under `oltpChanges`
//! (`FunctionCapabilities::oltp_changes`) and the run is production's
//! ([`signs`]): branches do not capture in phase 1, so anywhere else nothing
//! is sent. A failure is a platform error raised before the app's first
//! statement, as `name_session`'s is: the caller drops the transaction, which
//! rolls it back.

mod nudge;

use uuid::Uuid;

use super::super::env_policy::EnvPolicy;
use super::*;
use crate::oltp_changes::provenance::{self, ChangeSigner, SIGN_SQL, TXID_SQL};

/// Whether a transaction of this run is signed: its build declares a table to
/// capture, and it runs in production.
pub(super) fn signs(oltp_changes: bool, policy: &EnvPolicy) -> bool {
    oltp_changes && policy.is_production()
}

/// Who a transaction is signed as: everything but its id, which is read from
/// the transaction itself.
struct Signing<'a> {
    environment: &'a str,
    /// The writer's schema, `app_<writer>`.
    writer: &'a str,
    actor: &'a str,
    invocation: Option<Uuid>,
}

impl ProjectFunctionHost {
    /// Sign `tx`, just opened on the app's own schema `writer`
    /// (`app_<writer>`), for the change stream: a no-op unless
    /// `self.caps.oltp_changes` and the run is production's.
    pub(super) async fn sign_oltp_tx(
        &self,
        tx: &mut dyn SqlTransaction,
        writer: &str,
    ) -> Result<(), String> {
        if !signs(self.caps.oltp_changes, &self.policy) {
            return Ok(());
        }
        let environment = self.policy.environment().name();
        let actor = self.actor.to_string();
        let signing = Signing {
            environment: &environment,
            writer,
            actor: &actor,
            invocation: Some(self.identity.invocation_id),
        };
        let org_id = self.org_id;
        with_db_timeout(
            "sign",
            sign_tx(tx, &signing, |s| provenance::sign_hex(org_id, s)),
        )
        .await
    }
}

/// Read `tx`'s id ([`TXID_SQL`]), then set who made its changes
/// ([`SIGN_SQL`]), with the hex MAC `mac` computes over them. Only the MAC is
/// bound; the key stays inside `mac`.
async fn sign_tx(
    tx: &mut dyn SqlTransaction,
    signing: &Signing<'_>,
    mac: impl Fn(&ChangeSigner<'_>) -> String,
) -> Result<(), String> {
    let failed = |e: String| {
        format!("could not sign the transaction for the change stream (platform-side): {e}")
    };
    let rows = tx
        .query(TXID_SQL, &[])
        .await
        .map_err(|e| failed(e.to_string()))?;
    let txid = rows
        .first()
        .and_then(serde_json::Value::as_object)
        .and_then(|row| row.values().next())
        .and_then(serde_json::Value::as_str)
        .and_then(provenance::parse_txid)
        .ok_or_else(|| failed(format!("no transaction id in {rows:?}")))?;
    let signer = ChangeSigner {
        txid,
        environment: signing.environment,
        writer: signing.writer,
        actor: signing.actor,
        invocation: signing.invocation,
    };
    let params = [
        serde_json::Value::String(signing.actor.to_string()),
        // `''` is no invocation, as the capture trigger reads it.
        serde_json::Value::String(
            signing
                .invocation
                .map(|id| id.to_string())
                .unwrap_or_default(),
        ),
        serde_json::Value::String(mac(&signer)),
    ];
    tx.exec(SIGN_SQL, &params)
        .await
        .map(|_| ())
        .map_err(|e| failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use agentic_connector::{ConnectorError, SqlTransaction, TxParams};
    use oxy_app_core::custom_app_environment::AppEnvironment;

    use super::*;
    use crate::oltp_changes::provenance::{sign_keyed, verify_keyed};

    #[test]
    fn signing_is_skipped_outside_production() {
        assert!(signs(true, &EnvPolicy::production()));
        for environment in [
            AppEnvironment::Staging,
            AppEnvironment::Dev {
                handle: "ada".to_string(),
            },
        ] {
            let policy = EnvPolicy::for_environment(environment);
            assert!(!signs(true, &policy), "{:?}", policy.environment());
        }
    }

    #[test]
    fn signing_is_skipped_without_declared_tables() {
        assert!(!signs(false, &EnvPolicy::production()));
        assert!(!signs(
            false,
            &EnvPolicy::for_environment(AppEnvironment::Staging)
        ));
    }

    /// A transaction that answers [`TXID_SQL`] with `txid` and records every
    /// statement it was sent.
    struct Recording {
        txid: &'static str,
        sent: Vec<(String, Vec<serde_json::Value>)>,
    }

    #[async_trait::async_trait]
    impl SqlTransaction for Recording {
        async fn query(
            &mut self,
            sql: &str,
            params: &TxParams,
        ) -> Result<Vec<serde_json::Value>, ConnectorError> {
            self.sent.push((sql.to_string(), params.to_vec()));
            Ok(vec![serde_json::json!({ "pg_current_xact_id": self.txid })])
        }
        async fn exec(&mut self, sql: &str, params: &TxParams) -> Result<u64, ConnectorError> {
            self.sent.push((sql.to_string(), params.to_vec()));
            Ok(1)
        }
        async fn commit(self: Box<Self>) -> Result<(), ConnectorError> {
            Ok(())
        }
        async fn rollback(self: Box<Self>) -> Result<(), ConnectorError> {
            Ok(())
        }
    }

    const KEY: [u8; 32] = [3; 32];

    fn signing(invocation: Option<Uuid>) -> Signing<'static> {
        Signing {
            environment: "production",
            writer: "app_orders",
            actor: "user-1",
            invocation,
        }
    }

    /// The id is read first, then the three settings set from bound
    /// parameters, the MAC over that id and no other.
    #[tokio::test]
    async fn a_production_tx_is_signed_over_its_own_txid() {
        let invocation = Uuid::from_u128(5);
        let mut tx = Recording {
            txid: "742",
            sent: Vec::new(),
        };
        sign_tx(&mut tx, &signing(Some(invocation)), |s| {
            hex::encode(sign_keyed(&KEY, s))
        })
        .await
        .expect("signs");
        let [(read, none), (set, params)] = &tx.sent[..] else {
            panic!("two statements: {:?}", tx.sent);
        };
        assert_eq!((read.as_str(), none.len()), (TXID_SQL, 0));
        assert_eq!(set.as_str(), SIGN_SQL);
        assert_eq!(params[0], "user-1");
        assert_eq!(params[1], invocation.to_string());
        let mac = hex::decode(params[2].as_str().expect("hex text")).expect("hex");
        let signed = ChangeSigner {
            txid: 742,
            environment: "production",
            writer: "app_orders",
            actor: "user-1",
            invocation: Some(invocation),
        };
        assert!(verify_keyed(&KEY, &signed, Some(mac.as_slice())));
        let elsewhere = ChangeSigner {
            txid: 743,
            ..signed
        };
        assert!(!verify_keyed(&KEY, &elsewhere, Some(mac.as_slice())));
    }

    /// No transaction id, no settings: the caller drops the transaction.
    #[tokio::test]
    async fn a_tx_whose_id_cannot_be_read_is_not_signed() {
        let mut tx = Recording {
            txid: "",
            sent: Vec::new(),
        };
        let refused = sign_tx(&mut tx, &signing(None), |s| {
            hex::encode(sign_keyed(&KEY, s))
        })
        .await
        .expect_err("no id");
        assert!(refused.contains("change stream"), "{refused}");
        assert_eq!(tx.sent.len(), 1, "nothing is set: {:?}", tx.sent);
    }
}
