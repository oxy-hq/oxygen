//! A read-only mint is handed out only when Airhouse echoed the claim.

use chrono::Utc;
use uuid::Uuid;

use super::super::{AirhouseTokenBroker, BrokerError, BrokerSubject, CacheEntry, SystemPurpose};
use crate::admin::{AirhouseAdminClient, EphemeralCredential, UserRole};

const TTL: std::time::Duration = std::time::Duration::from_secs(900);

/// A broker whose Admin API is nowhere: a mint that reaches the network
/// fails with some other error, never the one a test asserts.
fn broker() -> AirhouseTokenBroker {
    AirhouseTokenBroker::new(AirhouseAdminClient::new("http://127.0.0.1:9", "tok"))
}

fn subject(ws: Uuid) -> BrokerSubject {
    BrokerSubject::System {
        workspace_id: ws,
        purpose: SystemPurpose::ReadOnlyQuery,
    }
}

fn cred(read_only: bool) -> EphemeralCredential {
    EphemeralCredential {
        username: "eph_ro".into(),
        password: "tk_ro".into(),
        tenant: "acme".into(),
        role: "reader".into(),
        expires_at: Utc::now() + chrono::Duration::seconds(3600),
        service_account_id: "sa_acme".into(),
        write_schemas: None,
        shared_session: true,
        uncached_reads: false,
        read_only,
    }
}

/// Put `cred` where a read-only mint for `ws` looks first.
async fn seed(broker: &AirhouseTokenBroker, ws: Uuid, cred: EphemeralCredential) {
    let key = broker.cache_key(ws, &subject(ws), UserRole::Reader);
    broker.cache.write().await.insert(key, CacheEntry { cred });
}

#[test]
fn only_a_read_only_query_asks_for_read_only_and_it_shares_its_session() {
    let ws = Uuid::nil();
    assert!(subject(ws).read_only());
    assert!(subject(ws).shared_session(), "its connection is pooled");
    assert_eq!(
        subject(ws).audit_subject(),
        format!("system:workspace:{ws}:read-only-query")
    );
    for purpose in [
        SystemPurpose::SharedQuery,
        SystemPurpose::Preview,
        SystemPurpose::TableWatch,
    ] {
        let other = BrokerSubject::System {
            workspace_id: ws,
            purpose,
        };
        assert!(!other.read_only(), "{purpose:?}");
    }
    assert!(!BrokerSubject::User(Uuid::nil()).read_only());
}

#[tokio::test]
async fn an_echoed_read_only_credential_is_handed_out() {
    let (broker, ws) = (broker(), Uuid::new_v4());
    seed(&broker, ws, cred(true)).await;
    let got = broker.mint_read_only(ws, TTL).await.expect("echoed");
    assert!(got.read_only);
}

/// An Airhouse older than the claim mints an ordinary Reader: it is evicted,
/// never handed out, and the next mint is refused without asking again.
#[tokio::test]
async fn a_reader_airhouse_did_not_mark_read_only_is_refused_and_remembered() {
    let (broker, ws) = (broker(), Uuid::new_v4());
    seed(&broker, ws, cred(false)).await;
    let err = broker
        .mint_read_only(ws, TTL)
        .await
        .expect_err("not echoed");
    assert!(matches!(err, BrokerError::ReadOnlyUnsupported), "{err}");
    assert!(
        broker.cache.read().await.is_empty(),
        "the credential was evicted"
    );

    // Seeded again, a good credential is still not reached: the refusal is
    // kept for a while rather than paid for on every react start.
    seed(&broker, ws, cred(true)).await;
    let err = broker
        .mint_read_only(ws, TTL)
        .await
        .expect_err("refusal kept");
    assert!(matches!(err, BrokerError::ReadOnlyUnsupported), "{err}");
}

/// Whatever entry point builds the subject, a read-only credential is a
/// Reader: anything else is refused before a mint.
#[tokio::test]
async fn a_read_only_credential_must_be_a_reader() {
    let (broker, ws) = (broker(), Uuid::new_v4());
    for role in [UserRole::Writer, UserRole::Admin] {
        let err = broker
            .mint_for_system(ws, SystemPurpose::ReadOnlyQuery, role, TTL)
            .await
            .expect_err("not a reader");
        assert!(matches!(err, BrokerError::ReadOnlyRole(_)), "{err}");
    }
}
