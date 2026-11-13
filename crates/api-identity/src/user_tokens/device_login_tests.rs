//! The device-code routes' decisions that need no database.

use serde_json::json;

use super::*;

#[test]
fn an_approval_is_only_ever_of_the_kind_asked_for() {
    let login = json!({});
    let agent = json!({ "kind": "agent", "hours": "4" });
    let sandbox = json!({ "kind": "sandbox_agent", "apps": "acme/store" });
    let agent_mint = json!({ "kind": "agent", "expires_in_hours": 4 });
    let sandbox_mint = json!({ "kind": "sandbox_agent", "apps": [], "expires_in_hours": 4 });

    assert!(mint_matches_ask(&login, None));
    assert!(mint_matches_ask(&agent, Some(&agent_mint)));
    assert!(mint_matches_ask(&sandbox, Some(&sandbox_mint)));

    // Never wider, never another kind, never a login for a mint.
    assert!(!mint_matches_ask(&login, Some(&agent_mint)));
    assert!(!mint_matches_ask(&agent, None));
    assert!(!mint_matches_ask(&sandbox, None));
    assert!(!mint_matches_ask(&sandbox, Some(&agent_mint)));
    assert!(!mint_matches_ask(&agent, Some(&sandbox_mint)));
}

#[test]
fn a_refused_approval_tells_the_cli_why() {
    let reason = denial_reason(&TokenError::AppNotFound("x".into()));
    assert!(
        reason.contains("may create sandbox agent tokens"),
        "{reason}"
    );
    let reason = denial_reason(&TokenError::ExceedsPolicy {
        max_lifetime_days: 1,
    });
    assert!(reason.contains("1-day"), "{reason}");
}

#[test]
fn a_poll_answers_in_the_exchanges_shape() {
    let pending = serde_json::to_value(PollResponse::Pending { interval: 5 }).unwrap();
    assert_eq!(pending, json!({ "status": "pending", "interval": 5 }));
    let slow = serde_json::to_value(PollResponse::SlowDown { interval: 5 }).unwrap();
    assert_eq!(slow, json!({ "status": "slow_down", "interval": 5 }));
}

#[test]
fn an_approval_by_device_code_never_asks_for_standing() {
    let agent =
        |standing: bool| json!({ "kind": "agent", "expires_in_hours": 4, "standing": standing });
    assert!(asks_for_standing(Some(&agent(true))));
    assert!(!asks_for_standing(Some(&agent(false))));
    assert!(!asks_for_standing(Some(
        &json!({ "kind": "sandbox_agent" })
    )));
    assert!(!asks_for_standing(None));
}
