use super::PublishCredential::{Other, PublishToken, SandboxAgent};
use super::*;

fn sandbox(handle: &str) -> AppEnvironment {
    AppEnvironment::Dev {
        handle: handle.into(),
    }
}

/// Absent or empty is today's publish, whatever else was sent — for every
/// credential but a sandbox agent token.
#[test]
fn no_environment_is_the_channels_publish() {
    for absent in [None, Some(""), Some("   ")] {
        for (promote, credential) in [
            (false, Other),
            (true, Other),
            (false, PublishToken),
            (true, PublishToken),
        ] {
            assert!(matches!(
                target_of(absent, promote, credential),
                Ok(PublishTarget::Channels)
            ));
        }
    }
}

#[test]
fn a_sandbox_name_selects_that_sandbox() {
    for credential in [Other, SandboxAgent] {
        let target = target_of(Some(" dev-a1 "), false, credential).expect("a sandbox");
        assert!(matches!(target, PublishTarget::Sandbox(env) if env == sandbox("a1")));
    }
}

/// Only a sandbox can be named: staging and production are reached by the
/// publish this field leaves alone.
#[test]
fn a_name_that_is_not_a_sandboxs_is_invalid() {
    for name in ["staging", "production", "dev-", "dev--x", "a1", "DEV-a1"] {
        for credential in [Other, SandboxAgent] {
            let refused = target_of(Some(name), false, credential).expect_err(name);
            assert!(
                matches!(&refused, PublishError::InvalidEnvironment(n) if n == name),
                "{name}: {refused}"
            );
        }
    }
}

/// The refusals in order: the name, then `promote`, then the credential.
#[test]
fn promote_and_a_publish_token_are_refused_with_a_sandbox() {
    assert!(matches!(
        target_of(Some("dev-a1"), true, Other),
        Err(PublishError::SandboxWithPromote)
    ));
    assert!(matches!(
        target_of(Some("dev-a1"), false, PublishToken),
        Err(PublishError::SandboxRefused)
    ));
    assert!(matches!(
        target_of(Some("dev-a1"), true, PublishToken),
        Err(PublishError::SandboxWithPromote)
    ));
    assert!(matches!(
        target_of(Some("nope"), true, PublishToken),
        Err(PublishError::InvalidEnvironment(_))
    ));
}

/// A sandbox agent token never reaches the channels: no `environment`, or
/// `promote` beside one, is `sandbox_token_refused` — `403`, with a code.
#[test]
fn a_sandbox_agent_token_is_refused_the_channels_and_promote() {
    for (environment, promote) in [
        (None, false),
        (None, true),
        (Some(""), false),
        (Some("  "), true),
        (Some("dev-a1"), true),
    ] {
        let refused = target_of(environment, promote, SandboxAgent).expect_err("refused");
        assert!(
            matches!(refused, PublishError::SandboxTokenRefused),
            "{environment:?} promote={promote}: {refused}"
        );
        assert_eq!(refused.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(refused.code(), Some("sandbox_token_refused"));
    }
}

/// With no branch, declared files are named — and so is how to get one.
#[test]
fn with_no_branch_declared_oltp_migrations_are_named_in_a_warning() {
    assert_eq!(oltp_warning(&sandbox("a1"), 0), None);
    let warning = oltp_warning(&sandbox("a1"), 2).expect("a warning");
    assert!(
        warning.starts_with("2 OLTP migration file(s) were not applied"),
        "{warning}"
    );
    assert!(warning.contains("dev-a1"), "{warning}");
    assert!(
        warning.contains("oxyc oltp provision --branch staging"),
        "{warning}"
    );
}
