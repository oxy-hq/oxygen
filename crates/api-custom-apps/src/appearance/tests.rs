use serde_json::json;

use super::{ColorMode, answer, parse_body};

#[test]
fn each_mode_round_trips_through_its_spelling() {
    for mode in [ColorMode::Light, ColorMode::Dark, ColorMode::System] {
        assert_eq!(ColorMode::parse(mode.as_str()), Some(mode));
    }
}

#[test]
fn only_the_three_spellings_parse() {
    for raw in ["", "Light", "DARK", "auto", "light ", "high-contrast"] {
        assert_eq!(ColorMode::parse(raw), None, "{raw:?}");
    }
}

#[test]
fn a_write_names_one_of_the_three_modes() {
    assert_eq!(parse_body(&json!({ "mode": "dark" })), Ok(ColorMode::Dark));
    assert_eq!(
        parse_body(&json!({ "mode": "system" })),
        Ok(ColorMode::System)
    );
    for body in [
        json!({}),
        json!({ "mode": null }),
        json!({ "mode": 1 }),
        json!({ "mode": "sepia" }),
        json!("dark"),
        json!(["dark"]),
    ] {
        assert!(parse_body(&body).is_err(), "{body}");
    }
}

#[test]
fn not_chosen_answers_null_and_a_choice_answers_its_name() {
    assert_eq!(answer(None), json!({ "mode": null }));
    assert_eq!(answer(Some(ColorMode::Light)), json!({ "mode": "light" }));
}

fn user(email: Option<&str>) -> oxy_auth::types::AuthenticatedUser {
    oxy_auth::types::AuthenticatedUser {
        id: uuid::Uuid::new_v4(),
        email: email.map(str::to_string),
        name: "Sam".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    }
}

#[test]
fn a_member_carries_a_mode_and_a_crew_sign_in_does_not() {
    assert!(super::has_mode(&user(Some("sam@example.com"))));
    // A frontline worker has no mailbox and no Oxygen account.
    assert!(!super::has_mode(&user(None)));
}
