use super::*;
use crate::test_fixtures::{
    DRAFT_YAML, FULL_YAML, SWITCHBACK_YAML, date, draft, full, switchback, switchback_draft,
};

/// The message of the parse error `yaml` earns; fails if it parses.
fn parse_error(yaml: &str) -> String {
    match parse(yaml) {
        Err(ConfigError::Parse { message }) => message,
        other => panic!("expected a parse error, got {other:?}"),
    }
}

/// Only the keys with defaults are omitted.
const MINIMAL: &str = r#"
experiment: m
hypothesis: h
owner: o
for_each: store
lever:
  on: { via: a }
  off: { via: b }
  read_back: { when_on: "1 == 1", when_off: "1 == 0", grace_days: 1 }
design: { timezone: UTC, pre_days: 7, post_days: 7 }
outcome: { target: t.m, direction: increase }
"#;

#[test]
fn the_specs_full_example_parses() {
    let c = full();
    assert_eq!(c.experiment, "beverage_promo_pilot");
    assert_eq!(c.for_each, "store");
    assert_eq!(
        c.assign_by, None,
        "the spec's example randomises single stores"
    );
    assert_eq!(c.lever.as_ref().unwrap().per, LeverPer::Unit);
    assert_eq!(c.lever.as_ref().unwrap().on.via, "promo_on");
    assert_eq!(c.lever.as_ref().unwrap().off.via, "promo_off");
    assert_eq!(c.lever.as_ref().unwrap().read_back.grace_days, 2);
    assert_eq!(
        c.lever.as_ref().unwrap().on_close,
        OnClose::Off,
        "`off` is a string, not a YAML boolean"
    );
    assert_eq!(c.design.timezone, chrono_tz::Australia::Sydney);
    assert_eq!(
        c.design.time_dimension.as_deref(),
        Some("store_days.business_date")
    );
    assert_eq!(c.design.waves.len(), 2);
    assert_eq!(c.design.waves[0].switch_on, date(2026, 3, 2));
    assert_eq!(c.design.waves[0].units, ["bondi", "newtown"]);
    assert_eq!(c.design.waves[1].units, ["manly"]);
    assert_eq!(c.design.switchback, None);
    assert_eq!(
        c.design.strata,
        Some(vec![
            vec![
                "bondi".into(),
                "manly".into(),
                "parramatta".into(),
                "chatswood".into()
            ],
            vec![
                "newtown".into(),
                "glebe".into(),
                "surry_hills".into(),
                "balmain".into()
            ],
        ])
    );
    assert_eq!(c.design.seed, Some(8841207));
    assert_eq!(
        (c.design.pre_days, c.design.post_days),
        (Some(56), Some(56))
    );
    assert_eq!((c.design.anticipation_days, c.design.washout_days), (3, 7));
    assert_eq!(
        (c.design.coverage_floor, c.design.compliance_floor),
        (0.9, 0.8)
    );
    assert_eq!(
        c.outcome.driver.as_deref(),
        Some("store_days.promo_redemptions")
    );
    assert_eq!(c.outcome.direction, Direction::Increase);
    assert_eq!(c.outcome.mde_at_proposal, Some(17.5));
    assert_eq!(
        c.outcome.prior_coefficient,
        Some(PriorCoefficient {
            value: 4.2,
            source: PriorSource::Fitted
        })
    );
    assert_eq!(
        c.substitution_check,
        Some(SubstitutionCheck {
            sibling: "store_days.entree_revenue".into(),
            filter: vec![],
        })
    );
    assert_eq!(c.guardrails.len(), 2);
    assert!(matches!(
        &c.guardrails[0].criteria,
        GuardrailCriteria::Anomaly { measure, significance }
            if measure == "store_days.refund_rate" && *significance == 0.95
    ));
    assert!(matches!(
        &c.guardrails[1].criteria,
        GuardrailCriteria::Trigger { .. }
    ));
    assert_eq!(c.guardrails[1].action, GuardrailAction::Abort);
}

#[test]
fn a_draft_without_waves_seed_or_mde_parses() {
    let c = draft();
    assert!(c.design.waves.is_empty());
    assert_eq!(c.design.seed, None);
    assert_eq!(c.outcome.mde_at_proposal, None);
    assert_eq!(
        c.design.time_dimension, None,
        "a draft without time_dimension still parses"
    );
    assert_eq!(c.design.strata, None);
    assert_eq!(c.outcome.prior_coefficient, None);
    assert_eq!(c.assign_by, None);
}

#[test]
fn the_switchback_example_parses() {
    let c = switchback();
    let sb = c.design.switchback.as_ref().expect("a switchback design");
    assert_eq!(sb.period_days, 7);
    assert_eq!(sb.periods.len(), 4);
    assert_eq!(
        sb.periods[0],
        PeriodConfig {
            from: date(2026, 3, 2).into(),
            state: LeverState::On
        }
    );
    assert_eq!(
        sb.periods[1].state,
        LeverState::Off,
        "`off` is a string, not a YAML boolean"
    );
    assert_eq!(sb.periods[3].from, date(2026, 3, 23));
    assert!(c.design.waves.is_empty() && c.design.strata.is_none());
    assert_eq!((c.design.pre_days, c.design.post_days), (None, None));
    assert_eq!(c.design.washout_days, 2);
    assert_eq!(c.lever.as_ref().unwrap().per, LeverPer::Wave);
    assert_eq!(c.outcome.alpha, 0.2);
}

#[test]
fn a_switchback_draft_has_a_period_length_and_no_periods() {
    let sb = switchback_draft()
        .design
        .switchback
        .expect("a switchback key");
    assert_eq!(sb.period_days, 7);
    assert!(
        sb.periods.is_empty(),
        "periods default to empty: pricing writes them"
    );
    assert_eq!(switchback_draft().design.seed, None);
}

#[test]
fn per_and_assign_by_parse_and_a_bad_per_is_an_error() {
    let wave =
        parse(&MINIMAL.replace("  off: { via: b }", "  off: { via: b }\n  per: wave")).unwrap();
    assert_eq!(wave.lever.as_ref().unwrap().per, LeverPer::Wave);
    let grouped =
        parse(&MINIMAL.replace("for_each: store", "for_each: sku\nassign_by: sku.category"))
            .unwrap();
    assert_eq!(grouped.assign_by.as_deref(), Some("sku.category"));
    let message =
        parse_error(&MINIMAL.replace("  off: { via: b }", "  off: { via: b }\n  per: batch"));
    assert!(message.contains("batch"), "{message}");
}

#[test]
fn an_unknown_key_in_a_switchback_period_or_a_prior_coefficient_is_an_error() {
    let message = parse_error(&SWITCHBACK_YAML.replace(
        "{ from: 2026-03-02, state: on }",
        "{ from: 2026-03-02, state: on, extra: 1 }",
    ));
    assert!(message.contains("extra"), "{message}");
    let message =
        parse_error(&SWITCHBACK_YAML.replace("period_days: 7", "period_days: 7\n    stride: 2"));
    assert!(message.contains("stride"), "{message}");
    let message = parse_error(&FULL_YAML.replace("source: fitted", "source: fitted, note: x"));
    assert!(message.contains("note"), "{message}");
    let message = parse_error(&FULL_YAML.replace("source: fitted", "source: guessed"));
    assert!(message.contains("guessed"), "{message}");
}

#[test]
fn an_unknown_top_level_key_is_an_error_naming_the_key() {
    let message = parse_error(&format!("{FULL_YAML}\nbogus_key: 1\n"));
    assert!(message.contains("bogus_key"), "{message}");
}

#[test]
fn a_typo_in_an_optional_key_is_an_error_not_a_draft() {
    // `seed` is optional, so without deny_unknown_fields `sead` would
    // silently turn a registrable file into a draft.
    let message = parse_error(&FULL_YAML.replace("  seed: 8841207", "  sead: 8841207"));
    assert!(message.contains("sead"), "{message}");
}

#[test]
fn an_unknown_key_inside_a_guardrail_is_an_error() {
    let yaml = FULL_YAML.replace("type: trigger,", "type: trigger, extra: 1,");
    let message = parse_error(&yaml);
    assert!(message.contains("extra"), "{message}");
}

#[test]
fn an_opportunity_guardrail_is_rejected() {
    let yaml = FULL_YAML.replace(
        "type: anomaly, measure: store_days.refund_rate, significance: 0.95",
        "type: opportunity, measure: store_days.refund_rate",
    );
    let message = parse_error(&yaml);
    assert!(message.contains("opportunity"), "{message}");
}

#[test]
fn on_close_defaults_to_off_and_keep_parses() {
    assert!(
        !DRAFT_YAML.contains("on_close"),
        "the draft fixture must omit on_close"
    );
    assert_eq!(draft().lever.as_ref().unwrap().on_close, OnClose::Off);
    let kept = parse(&FULL_YAML.replace("on_close: off", "on_close: keep")).unwrap();
    assert_eq!(kept.lever.as_ref().unwrap().on_close, OnClose::Keep);
}

#[test]
fn omitted_keys_take_the_contracts_defaults() {
    let c = parse(MINIMAL).unwrap();
    assert_eq!((c.design.anticipation_days, c.design.washout_days), (0, 0));
    assert_eq!(
        (c.design.coverage_floor, c.design.compliance_floor),
        (0.9, 0.8)
    );
    assert_eq!(c.outcome.family, 1);
    assert_eq!((c.outcome.alpha, c.outcome.power), (0.05, 0.80));
    assert!(c.guardrails.is_empty());
    assert_eq!(c.substitution_check, None);
    assert_eq!(c.outcome.driver, None);
    assert_eq!(c.lever.as_ref().unwrap().per, LeverPer::Unit);
    assert_eq!(
        (c.assign_by, c.design.strata, c.design.switchback),
        (None, None, None)
    );
    assert_eq!(c.outcome.prior_coefficient, None);
}

#[test]
fn an_unquoted_scalar_unit_is_kept_verbatim() {
    // A unit is a string and serde_yaml reads any plain scalar into a `String`
    // as its source text, so `12` is "12" and `007` stays "007" — never 7.
    let yaml = FULL_YAML.replace("units: [bondi, newtown]", "units: [12, 007, newtown]");
    let c = parse(&yaml).unwrap();
    assert_eq!(c.design.waves[0].units, ["12", "007", "newtown"]);
}

#[test]
fn a_missing_required_key_is_an_error_naming_it() {
    let message = parse_error(&MINIMAL.replace("experiment: m\n", ""));
    assert!(message.contains("experiment"), "{message}");
}

#[test]
fn an_unknown_timezone_is_an_error() {
    let message = parse_error(&MINIMAL.replace("timezone: UTC", "timezone: Mars/Olympus"));
    assert!(message.contains("timezone"), "{message}");
}

#[test]
fn config_round_trips_through_json_for_every_fixture() {
    for c in [full(), draft(), switchback(), switchback_draft()] {
        let value = serde_json::to_value(&c).unwrap();
        assert_eq!(value["design"]["timezone"], "Australia/Sydney");
        assert_eq!(value["lever"]["on_close"], "off");
        let back: ExperimentConfig = serde_json::from_value(value).unwrap();
        assert_eq!(back, c);
    }
    let value = serde_json::to_value(draft()).unwrap();
    assert!(
        value["design"]["seed"].is_null(),
        "a draft's absent keys are null, not missing"
    );
    assert!(value["design"]["time_dimension"].is_null());
    assert!(value["design"]["strata"].is_null() && value["design"]["switchback"].is_null());
    assert!(value["outcome"]["prior_coefficient"].is_null() && value["assign_by"].is_null());
    assert_eq!(
        value["lever"]["per"], "unit",
        "the default is written out, not left to the reader"
    );

    let value = serde_json::to_value(full()).unwrap();
    assert_eq!(value["design"]["waves"][0]["switch_on"], "2026-03-02");
    assert_eq!(
        value["design"]["time_dimension"],
        "store_days.business_date"
    );
    assert_eq!(value["design"]["strata"][1][0], "newtown");
    assert_eq!(
        value["outcome"]["prior_coefficient"],
        serde_json::json!({"value": 4.2, "source": "fitted"})
    );

    let value = serde_json::to_value(switchback()).unwrap();
    assert_eq!(value["lever"]["per"], "wave");
    assert_eq!(
        value["design"]["switchback"]["periods"][0],
        serde_json::json!({"from": "2026-03-02", "state": "on"})
    );
    assert_eq!(value["design"]["switchback"]["period_days"], 7);
    assert!(
        value["design"]["pre_days"].is_null(),
        "an absent window is null, not 0"
    );
    let value = serde_json::to_value(switchback_draft()).unwrap();
    assert_eq!(
        value["design"]["switchback"]["periods"],
        serde_json::json!([])
    );
}

#[test]
fn measure_filters_parse_and_an_unfiltered_design_serializes_as_before() {
    let yaml = r#"
experiment: ads_hero_campaign
hypothesis: "Ads on one product lift its sales"
owner: growth
for_each: market
lever:
  on: { via: ads_on }
  off: { via: ads_off }
  read_back: { when_on: "{{ad_days.impressions}} > 0", when_off: "{{ad_days.impressions}} == 0", grace_days: 2 }
design:
  timezone: UTC
  pre_days: 28
  post_days: 28
outcome:
  target: sku_days.revenue
  target_filter:
    - { dimension: sku_days.sku, values: [TB-40] }
  driver: ad_days.spend
  direction: increase
substitution_check:
  sibling: sku_days.revenue
  filter:
    - { dimension: sku_days.category, values: [backpacks] }
    - { dimension: sku_days.sku, op: not_equals, values: [TB-40] }
"#;
    let c = parse(yaml).expect("parses");
    assert_eq!(
        c.outcome.target_filter,
        vec![MeasureFilter {
            dimension: "sku_days.sku".into(),
            op: FilterOp::Equals,
            values: vec!["TB-40".into()],
        }]
    );
    assert!(c.outcome.driver_filter.is_empty());
    let sibling = c.substitution_check.expect("sibling");
    assert_eq!(sibling.filter[1].op, FilterOp::NotEquals);

    // An unfiltered design must serialize byte-for-byte as before: its content
    // hash is how a frozen design is recognised.
    let value = serde_json::to_value(full()).unwrap();
    assert!(value["outcome"].get("target_filter").is_none());
    assert!(value["outcome"].get("driver_filter").is_none());
    assert!(value["substitution_check"].get("filter").is_none());
}

#[test]
fn an_unknown_filter_op_is_an_error() {
    let err = parse_error(
        &FULL_YAML.replace(
            "  direction: increase",
            "  direction: increase\n  target_filter: [{ dimension: store_days.region, op: like, values: [n] }]",
        ),
    );
    assert!(err.contains("like"), "{err}");
}

#[test]
fn a_switch_at_a_local_hour_parses_and_a_dated_design_serializes_as_before() {
    let yaml = FULL_YAML.replacen("switch_on: 2026-03-02", "switch_on: 2026-03-02T14:00", 1);
    let c = parse(&yaml).unwrap();
    assert_eq!(c.design.waves[0].switch_on.to_string(), "2026-03-02T14:00");
    let value = serde_json::to_value(&c).unwrap();
    assert_eq!(value["design"]["waves"][0]["switch_on"], "2026-03-02T14:00");
    assert_eq!(value["design"]["waves"][1]["switch_on"], "2026-03-16");
}

/// YAML 1.1 readers (PyYAML) turn an unquoted `on:` into `true` and `off:` into
/// `false`; the JSON they produce then names the keys "true" and "false".
#[test]
fn a_lever_whose_on_and_off_keys_became_booleans_still_parses() {
    let mut v = serde_json::to_value(full()).unwrap();
    let lever = v["lever"].as_object_mut().unwrap();
    let on = lever.remove("on").unwrap();
    let off = lever.remove("off").unwrap();
    lever.insert("true".into(), on);
    lever.insert("false".into(), off);
    let c: ExperimentConfig = serde_json::from_value(v).expect("aliases parse");
    assert_eq!(c, full());
}
