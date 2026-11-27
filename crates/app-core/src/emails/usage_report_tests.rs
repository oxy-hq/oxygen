use super::*;

#[test]
fn a_preview_deployment_previews_and_an_unconfigured_one_is_off() {
    assert_eq!(mode_for(true, false), DeliveryMode::Email);
    assert_eq!(mode_for(true, true), DeliveryMode::Preview);
    assert_eq!(mode_for(false, true), DeliveryMode::Preview);
    assert_eq!(mode_for(false, false), DeliveryMode::Off);
    assert_eq!(
        serde_json::to_value(DeliveryMode::Off).unwrap(),
        serde_json::json!("off")
    );
}
