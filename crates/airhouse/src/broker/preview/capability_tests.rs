//! The capabilities probe: bounded, never held under the broker's lock, and a
//! "no" (or a Writer minted unconfined) forgets every cached preview
//! credential, so none minted before a downgrade is handed out again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::{
    AirhouseTokenBroker, BrokerError, BrokerSubject, CacheEntry, CacheKey, SystemPurpose,
};
use super::tests::{TTL, cred, ns, schema, scopes_writers, seed};
use crate::admin::{AirhouseAdminClient, UserRole};

fn broker_at(server: &MockServer) -> AirhouseTokenBroker {
    AirhouseTokenBroker::new(AirhouseAdminClient::new(server.uri(), "tok"))
}

async fn mount_capabilities(server: &MockServer, answer: ResponseTemplate, times: u64) {
    Mock::given(method("GET"))
        .and(path("/admin/v1/capabilities"))
        .respond_with(answer)
        .expect(times)
        .mount(server)
        .await;
}

/// Cache entries of subjects that are not a preview's, a preview's own
/// live-table reads and TTL drops included: a downgrade must not touch them.
async fn seed_others(broker: &AirhouseTokenBroker, ws: Uuid) -> Vec<CacheKey> {
    let others = [
        (
            BrokerSubject::System {
                workspace_id: ws,
                purpose: SystemPurpose::Preview,
            },
            UserRole::Reader,
        ),
        (
            BrokerSubject::System {
                workspace_id: ws,
                purpose: SystemPurpose::PreviewDdl,
            },
            UserRole::Writer,
        ),
        (
            BrokerSubject::App {
                workspace_id: ws,
                app_slug: "preview".into(),
                schema: "app_preview".into(),
            },
            UserRole::Writer,
        ),
        (BrokerSubject::User(Uuid::new_v4()), UserRole::Reader),
    ];
    let mut keys = Vec::new();
    for (subject, role) in others {
        let key = broker.cache_key(ws, &subject, role);
        let entry = CacheEntry {
            cred: cred(role.as_str(), None),
        };
        broker.cache.write().await.insert(key.clone(), entry);
        keys.push(key);
    }
    keys
}

async fn cached_keys(broker: &AirhouseTokenBroker) -> Vec<CacheKey> {
    let mut keys: Vec<_> = broker.cache.read().await.keys().cloned().collect();
    keys.sort_by(|a, b| a.1.cmp(&b.1));
    keys
}

fn sorted(mut keys: Vec<CacheKey>) -> Vec<CacheKey> {
    keys.sort_by(|a, b| a.1.cmp(&b.1));
    keys
}

/// A probe that gets no answer in time is an error, keeps nothing (the next
/// mint asks again), and holds no lock while it waits.
#[tokio::test]
async fn an_unanswered_probe_is_an_error_that_keeps_nothing() {
    let server = MockServer::start().await;
    let slow = ResponseTemplate::new(200)
        .set_body_json(json!({"mint.write_schemas": true}))
        .set_delay(Duration::from_secs(30));
    mount_capabilities(&server, slow, 2).await;
    let mut broker = broker_at(&server);
    broker.scoped_writers.probe_within = Duration::from_millis(500);
    let broker = Arc::new(broker);
    let (ws, scope) = (Uuid::new_v4(), vec![schema("toast_pos")]);

    for _ in 0..2 {
        let started = Instant::now();
        let probing = tokio::spawn({
            let (broker, scope) = (Arc::clone(&broker), scope.clone());
            async move {
                broker
                    .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            broker.scoped_writers.known.try_lock().is_ok(),
            "the probe holds the lock while it waits"
        );
        let err = probing.await.unwrap().expect_err("an unanswered probe");
        assert!(
            matches!(err, BrokerError::CapabilitiesUnanswered(_)),
            "{err}"
        );
        assert!(started.elapsed() < Duration::from_secs(10), "not bounded");
        assert!(broker.scoped_writers.known.lock().unwrap().is_none());
    }
}

/// The probe's "no" refuses the Writer, cached or not, and forgets every
/// cached preview credential; nobody else's.
#[tokio::test]
async fn a_no_forgets_every_cached_preview_credential() {
    let server = MockServer::start().await;
    let no = ResponseTemplate::new(200).set_body_json(json!({"mint.write_schemas": false}));
    mount_capabilities(&server, no, 1).await;
    let broker = broker_at(&server);
    let (ws, scope) = (Uuid::new_v4(), vec![schema("toast_pos")]);
    let writer = cred("writer", Some(scope.clone()));
    seed(&broker, ws, &scope, UserRole::Writer, writer).await;
    seed(&broker, ws, &[], UserRole::Reader, cred("reader", None)).await;
    let others = seed_others(&broker, ws).await;

    let err = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect_err("a Writer where Airhouse says it cannot confine one");
    assert!(
        matches!(err, BrokerError::ScopedWritersUnsupported),
        "{err}"
    );
    assert_eq!(cached_keys(&broker).await, sorted(others));
}

/// A Writer minted unconfined is a downgrade seen at the mint: every cached
/// preview credential is forgotten with it, not only the refused one.
#[tokio::test]
async fn an_unconfined_writer_forgets_every_cached_preview_credential() {
    let broker = AirhouseTokenBroker::new(AirhouseAdminClient::new("http://127.0.0.1:9", "tok"));
    scopes_writers(&broker, true, Duration::ZERO).await;
    let ws = Uuid::new_v4();
    let (confined, unconfined) = (vec![schema("toast_pos")], vec![schema("site_selection")]);
    let good = cred("writer", Some(confined.clone()));
    seed(&broker, ws, &confined, UserRole::Writer, good).await;
    seed(
        &broker,
        ws,
        &unconfined,
        UserRole::Writer,
        cred("writer", None),
    )
    .await;
    seed(&broker, ws, &[], UserRole::Reader, cred("reader", None)).await;
    let others = seed_others(&broker, ws).await;

    let err = broker
        .mint_for_preview(ws, &ns(), &unconfined, UserRole::Writer, TTL)
        .await
        .expect_err("a Writer Airhouse did not confine");
    assert!(matches!(err, BrokerError::UnscopedWriter { .. }), "{err}");
    assert_eq!(cached_keys(&broker).await, sorted(others));
}

/// A Writer minted unconfined also un-keeps a cached "yes": without it,
/// `require_scoped_writers` trusts the stale "yes" with no expiry, so every
/// later preview write would pay a fresh mint (and revoke) round trip before
/// failing anyway, for as long as the downgrade lasts. With the fix, a later
/// mint fails fast on the cached "no" instead — so the capabilities endpoint
/// here is asked exactly once, by the probe that produced the "yes" the
/// first mint relied on.
#[tokio::test]
async fn an_unconfined_writer_makes_a_later_mint_short_circuit() {
    let server = MockServer::start().await;
    let yes = ResponseTemplate::new(200).set_body_json(json!({"mint.write_schemas": true}));
    mount_capabilities(&server, yes, 1).await;
    let broker = broker_at(&server);
    let ws = Uuid::new_v4();
    let scope = vec![schema("toast_pos")];
    // Cached as if already minted, but unconfined — Airhouse's own proof that
    // it does not scope Writers, surfacing the first time this scope is asked
    // for rather than at the mint that actually produced it.
    seed(&broker, ws, &scope, UserRole::Writer, cred("writer", None)).await;

    let err = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect_err("a Writer Airhouse did not confine");
    assert!(matches!(err, BrokerError::UnscopedWriter { .. }), "{err}");

    // The second attempt must not re-probe or re-mint: `mount_capabilities`'s
    // `expect(1)` above is the whole budget for this test, so a second GET
    // would fail it at teardown.
    let err = broker
        .mint_for_preview(ws, &ns(), &scope, UserRole::Writer, TTL)
        .await
        .expect_err("the deployment is known not to confine Writers");
    assert!(
        matches!(err, BrokerError::ScopedWritersUnsupported),
        "{err}"
    );
}

/// What a host asks before it prepares any preview write is the mint's own
/// answer: asked once and kept for a yes, a no read as a no, and a 404 (an
/// Airhouse older than the endpoint) as a no too.
#[tokio::test]
async fn the_hosts_question_is_the_mints_answer() {
    for (answer, expected) in [
        (
            ResponseTemplate::new(200).set_body_json(json!({"mint.write_schemas": true})),
            true,
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({"mint.write_schemas": false})),
            false,
        ),
        (ResponseTemplate::new(404), false),
    ] {
        let server = MockServer::start().await;
        mount_capabilities(&server, answer, 1).await;
        let broker = broker_at(&server);
        for _ in 0..2 {
            assert_eq!(broker.scopes_preview_writers().await.unwrap(), expected);
        }
        if !expected {
            let scope = [schema("toast_pos")];
            let err = broker
                .mint_for_preview(Uuid::new_v4(), &ns(), &scope, UserRole::Writer, TTL)
                .await
                .expect_err("the mint gives the same answer");
            assert!(
                matches!(err, BrokerError::ScopedWritersUnsupported),
                "{err}"
            );
        }
    }
}
