use crate::ConfigError;

#[test]
fn composite_key_refusal_names_the_entity_and_every_key() {
    let refusal = ConfigError::CompositeKey {
        entity: "store".to_string(),
        keys: vec!["store_id".to_string(), "region".to_string()],
    };
    let message = refusal.to_string();
    assert!(message.contains("`for_each: store`"), "{message}");
    assert!(message.contains("store_id, region"), "{message}");
}

#[test]
fn a_non_additive_assign_by_target_is_refused_naming_the_measure_and_its_aggregate() {
    let refusal = ConfigError::AssignByNotAdditive {
        measure: "sku_days.avg_price".to_string(),
        aggregate: "AVG".to_string(),
    };
    let message = refusal.to_string();
    assert!(message.contains("`sku_days.avg_price`"), "{message}");
    assert!(message.contains("AVG"), "{message}");
    assert!(message.contains("sum- or count-typed"), "{message}");
}

#[test]
fn an_unresolved_assign_by_names_the_key_and_the_reason() {
    let refusal = ConfigError::AssignByUnresolved {
        assign_by: "sku.categroy".to_string(),
        why: "`skus` declares no dimension `categroy`".to_string(),
    };
    let message = refusal.to_string();
    assert!(message.contains("`assign_by: sku.categroy`"), "{message}");
    assert!(
        message.contains("declares no dimension `categroy`"),
        "{message}"
    );
}
