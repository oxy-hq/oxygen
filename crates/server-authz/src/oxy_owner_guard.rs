//! `OxyOwnerGuard` — Oxy-staff-only gate for `/admin/*`.
//!
//! Reads the authenticated user (inserted upstream by `auth_middleware`)
//! and checks the email against the comma-separated `OXY_OWNER` allow-list
//! (case-insensitive, whitespace-trimmed). Returns `403 FORBIDDEN` if
//! `OXY_OWNER` is unset/empty or the caller's email isn't allowed, and
//! `401 UNAUTHORIZED` if no authenticated user is present.
//!
//! Applied as a router layer in `router::global` so all `/admin/*`
//! endpoints are gated by default — handlers don't need to repeat the
//! check, and new admin routes are guarded automatically.

use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use crate::caller::Caller;

pub async fn oxy_owner_guard_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let caller = Caller::from_extensions(request.extensions()).ok_or(StatusCode::UNAUTHORIZED)?;
    // Root as the CREDENTIAL carries it: an API token without `platform`, or one
    // narrowed to a list of orgs, is not root whoever its bearer is. An account
    // with no address can never hold it either — `is_oxy_owner` refuses a blank
    // needle outright; see the note there.
    if !crate::globals::is_global_owner(&caller) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}

/// Returns `true` when `email` matches the `OXY_OWNER` allow-list.
///
/// **About an address, not a request.** A request's standing depends on the
/// credential it arrived with, so anything deciding or displaying for the
/// requester goes through `globals::is_global_owner(&Caller)`.
///
/// Same matching rules as the middleware (case- and whitespace-insensitive,
/// comma-separated). Used by login responses to expose `is_owner` on the
/// user payload so the frontend can route owners to the admin shell. The
/// server-side middleware remains the authoritative gate for `/admin/*`.
pub fn is_oxy_owner(email: &str) -> bool {
    let needle = email.trim().to_ascii_lowercase();
    // A blank needle is never an owner.
    //
    // Not defensive padding — without it, `OXY_OWNER="a@b.com,"` (a stray
    // trailing comma, or any blank entry) yields an empty allow-list element
    // that an empty email matches exactly, and the caller is root. That was
    // unreachable while `users.email` was NOT NULL; it stopped being
    // unreachable when frontline identity made the address optional and
    // callers began passing `unwrap_or("")`. `platform_grant_checked` already
    // short-circuits a blank key for the same reason.
    if needle.is_empty() {
        return false;
    }
    oxy_owner_emails().contains(&needle)
}

/// The `OXY_OWNER` allow-list as addresses: lowercased, trimmed, blanks dropped,
/// in the order listed.
///
/// **For addressing people, never for deciding.** The one caller outside this
/// file is `globals::staff_holding`, which lists who a platform notification
/// goes to.
pub fn oxy_owner_emails() -> Vec<String> {
    std::env::var("OXY_OWNER")
        .unwrap_or_default()
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect()
}

#[cfg(test)]
fn require_oxy_owner(email: &str) -> Result<(), StatusCode> {
    if is_oxy_owner(email) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_email_never_matches_a_blank_allow_list_entry() {
        // The trailing comma is the whole point: it produces an empty element,
        // which an empty needle matches exactly. A frontline user has no
        // address, so callers pass "" — and without the blank guard this makes
        // them Oxy's root.
        let _g = EnvGuard::set("OXY_OWNER", "real@oxy.tech,");
        assert!(!is_oxy_owner(""));
        assert!(!is_oxy_owner("   "));
        assert!(
            is_oxy_owner("real@oxy.tech"),
            "the real owner still matches"
        );
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn the_allow_list_reads_as_clean_addresses_in_order() {
        let _g = EnvGuard::set("OXY_OWNER", " Root@Oxy.Tech ,, second@oxy.tech,");
        assert_eq!(oxy_owner_emails(), ["root@oxy.tech", "second@oxy.tech"]);
        let _g = EnvGuard::unset("OXY_OWNER");
        assert!(oxy_owner_emails().is_empty());
    }

    #[test]
    fn forbids_when_env_unset() {
        let _g = EnvGuard::unset("OXY_OWNER");
        assert_eq!(
            require_oxy_owner("anyone@example.com"),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn forbids_non_owner() {
        let _g = EnvGuard::set("OXY_OWNER", "owner@oxygen-hq.com");
        assert_eq!(
            require_oxy_owner("intruder@example.com"),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn allows_owner_case_and_whitespace_insensitive() {
        let _g = EnvGuard::set("OXY_OWNER", " Owner@Oxygen-Hq.Com , other@oxygen-hq.com ");
        assert_eq!(require_oxy_owner("owner@oxygen-hq.com"), Ok(()));
        assert_eq!(require_oxy_owner("OTHER@OXYGEN-HQ.COM"), Ok(()));
    }
}
