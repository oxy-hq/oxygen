//! `OxyContext.getPolicy` reading `policy_overrides` through the async op, as
//! the app's OLTP writer.

use oxy_policy_engine::{EngineError, PolicyDecision};

use crate::common::{self, SPRING_SALE};

#[tokio::test]
async fn the_script_decides_with_the_policy_stored_in_postgres() {
    let t = common::start().await;
    let mut engine = common::engine(&t).await;
    let decision = engine
        .decide("spring_sale", "SKU-1001")
        .await
        .expect("decides");
    assert_eq!(decision.policy_id, "spring_sale");
    assert!(
        ["control", "deep_discount"].contains(&decision.variant.as_str()),
        "a variant only the stored policy has: {decision:?}"
    );
    assert_eq!(engine.audit_log(), vec![decision]);
}

#[tokio::test]
async fn a_changed_row_is_read_on_the_next_decision() {
    let t = common::start().await;
    let mut engine = common::engine(&t).await;
    engine
        .decide("spring_sale", "SKU-1")
        .await
        .expect("decides");

    // One variant, so every SKU must land on it if the new row is read.
    let only = SPRING_SALE
        .replace(
            r#"{ "name": "control", "weight": 1, "priceMultiplier": 1.0 },"#,
            "",
        )
        .replace("deep_discount", "only_variant");
    common::upsert(&t.app, "spring_sale", &only).await;

    let decision = engine
        .decide("spring_sale", "SKU-1")
        .await
        .expect("decides");
    assert_eq!(
        decision,
        PolicyDecision {
            policy_id: "spring_sale".into(),
            sku: "SKU-1".into(),
            variant: "only_variant".into(),
            price_multiplier: 0.8,
        }
    );
}

#[tokio::test]
async fn a_missing_row_rejects_in_the_script() {
    let t = common::start().await;
    let mut engine = common::engine(&t).await;
    let err = engine.decide("nope", "SKU-1").await.expect_err("no row");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("unknown policy: nope")),
        "{err}"
    );
}

#[tokio::test]
async fn a_malformed_row_is_refused_before_it_crosses() {
    let t = common::start().await;
    let extra = SPRING_SALE.replacen('{', r#"{ "surprise": 1,"#, 1);
    common::upsert(&t.app, "spring_sale", &extra).await;
    let wrong_id = SPRING_SALE.replace(r#""id": "spring_sale""#, r#""id": "other""#);
    common::upsert(&t.app, "mislabelled", &wrong_id).await;

    let mut engine = common::engine(&t).await;
    let err = engine
        .decide("spring_sale", "SKU-1")
        .await
        .expect_err("extra field");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("policy spring_sale is malformed") && m.contains("surprise")),
        "{err}"
    );
    let err = engine
        .decide("mislabelled", "SKU-1")
        .await
        .expect_err("wrong id");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("its body names policy other")),
        "{err}"
    );
    assert!(engine.audit_log().is_empty(), "nothing was decided");
}

#[tokio::test]
async fn an_unreachable_store_rejects_without_leaking_the_password() {
    let t = common::start().await;
    let mut engine = common::engine(&t).await;
    t.container.stop().await.expect("stop the database");

    let err = engine
        .decide("spring_sale", "SKU-1")
        .await
        .expect_err("store is down");
    let msg = err.to_string();
    assert!(msg.contains("policy store read failed"), "{msg}");
    let encoded = oxy_oltp::roles::encode_userinfo(&t.writer_password);
    assert!(
        !msg.contains(&t.writer_password) && !msg.contains(&encoded),
        "{msg}"
    );
}

/// A `policy_overrides` outside the writer's schema is invisible to it: the
/// read resolves through the pinned `search_path`, not whatever table of that
/// name the database happens to hold.
#[tokio::test]
async fn a_table_outside_the_writers_schema_is_never_read() {
    let t = common::start().await;
    let decoy = SPRING_SALE.replace("spring_sale", "decoy");
    t.admin
        .batch_execute(&format!(
            "CREATE TABLE public.policy_overrides (LIKE {}.policy_overrides);
             INSERT INTO public.policy_overrides (policy_id, policy, updated_at)
             VALUES ('decoy', '{decoy}'::jsonb, now());
             GRANT SELECT ON public.policy_overrides TO PUBLIC",
            common::SCHEMA
        ))
        .await
        .expect("plant a decoy in public");

    let mut engine = common::engine(&t).await;
    let err = engine.decide("decoy", "SKU-1").await.expect_err("not ours");
    assert!(
        matches!(&err, EngineError::Script(m) if m.contains("unknown policy: decoy")),
        "{err}"
    );
}
