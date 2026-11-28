//! **The projection** from an authenticated request to the plain parts its
//! authorization caller is made of.
//!
//! Authentication knows its own credential, so it owns the one copy of this
//! mapping. `oxy_server_authz::Caller::of` builds its caller from it, and a
//! domain that authenticates a request itself (the custom-app gate, say)
//! builds the enforcement port's caller from it with
//! `contracts_authz::Caller::from_parts(caller_parts(..), authz)`. Moved here from
//! `oxy_server_authz::Caller::of` (C3a, 2026-11-28) with its body unchanged.

use oxy_authz::{CallerAccount, CallerParts, CallerToken, StoredKind, UnixTime};

use crate::token::CredentialContext;
use crate::types::AuthenticatedUser;

/// The parts of the caller of a request: its authenticated user and the key or
/// token it used, if any. A legacy credential narrows nothing, so it reads as a
/// session does.
///
/// `credential` is the request's `CredentialContext` marker when the call site
/// has it in hand; otherwise the one the user carries is used — they are the
/// same value, set together by the auth entry points.
///
/// The token's expiry becomes a [`UnixTime`], and a configured service's
/// standing is read from its entry here, once.
pub fn caller_parts(
    user: &AuthenticatedUser,
    credential: Option<&CredentialContext>,
) -> CallerParts {
    let credential = credential.or(user.credential.as_ref());
    let token = credential.and_then(|c| {
        c.reach().map(|reach| CallerToken {
            id: c.token_id,
            reach,
            expires_at: c.expires_at.map(|at| UnixTime {
                secs: at.timestamp(),
                subsec_nanos: at.timestamp_subsec_nanos(),
            }),
        })
    });
    let account = credential
        .filter(|c| c.is_service_account())
        .map(|c| CallerAccount {
            standing: c.service_account,
        });
    let app_publish = credential
        .map(|c| c.app_publish.clone())
        .unwrap_or_default();
    let service = credential
        .filter(|c| c.kind == StoredKind::Service)
        .and_then(|c| crate::token::service::standing_of(c.token_id));
    CallerParts {
        user_id: user.id,
        email: user.email.clone().unwrap_or_default(),
        token,
        account,
        app_publish,
        service,
    }
}
