//! Boot-time refusal: a fleet process must not start without a sign-in
//! provider.
//!
//! With no provider configured (no Google, no Okta, no magic link),
//! `oxy_auth`'s authenticator treats every browser request as the zero-config
//! guest user (`oxy_auth::built_in::set_auth_configured(false)`). That is the
//! right default for one process on a laptop and the wrong one for a
//! deployment: a missing environment variable would make every caller the same
//! signed-in user instead of failing.
//!
//! `OXY_ROLE=all` (what `oxy serve` is without an `OXY_ROLE`, and what
//! `oxy start` runs) keeps the guest fallback, and so does `--local`, whose
//! whole point is the guest. Every other role (`ide`, `serve`, `worker`) refuses
//! to start.

use oxy_app_core::process_role::Role;

/// Whether at least one sign-in provider (Google, Okta or magic link) is
/// configured, read the same way the authenticator's flag is set in `serve`.
pub fn provider_configured() -> bool {
    oxy::config::oxy::get_oxy_config()
        .ok()
        .and_then(|c| c.authentication)
        .map(|a| a.google.is_some() || a.okta.is_some() || a.magic_link.is_some())
        .unwrap_or(false)
}

/// `Err(message)` when `role` is not the single process, `local` is off, and no
/// provider is configured. Inputs are passed in so the rule is testable without
/// the process environment.
pub fn check(role: Role, local: bool, provider_configured: bool) -> Result<(), String> {
    if role == Role::All || local || provider_configured {
        return Ok(());
    }
    Err(format!(
        "OXY_ROLE={} needs a sign-in provider, and none is configured.\n\
         Set one of: MAGIC_LINK_FROM_EMAIL (magic link), GOOGLE_CLIENT_ID + \
         GOOGLE_CLIENT_SECRET, or OKTA_CLIENT_ID + OKTA_CLIENT_SECRET + OKTA_DOMAIN. \
         For local development, MAGIC_LINK_LOCAL_TEST=true.\n\n\
         Without one, every request would run as the guest user. A single process \
         (OXY_ROLE=all, or `oxy start`) keeps that fallback.",
        role.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_roles_refuse_without_a_provider() {
        for role in [Role::Ide, Role::Serve, Role::Worker] {
            let err = check(role, false, false).expect_err("must refuse");
            assert!(err.contains(role.as_str()), "{err}");
            assert!(err.contains("MAGIC_LINK_FROM_EMAIL"), "{err}");
        }
    }

    #[test]
    fn fleet_roles_start_with_a_provider() {
        for role in [Role::Ide, Role::Serve, Role::Worker] {
            assert_eq!(check(role, false, true), Ok(()));
        }
    }

    #[test]
    fn the_single_process_keeps_the_guest_fallback() {
        assert_eq!(check(Role::All, false, false), Ok(()));
    }

    #[test]
    fn local_mode_keeps_the_guest_fallback() {
        assert_eq!(check(Role::Serve, true, false), Ok(()));
    }
}
