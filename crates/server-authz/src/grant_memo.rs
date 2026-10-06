//! One request's platform grant, read once (sandbox agent credential design
//! §4, "Cost").
//!
//! A sandbox agent token's standing is read **uncached**: its minter losing
//! `develop_apps` must stop the token's next request, not one a 60 s cache
//! window later. A console request asks for that standing five or six times —
//! the owner-or-admin guard, the capability guard, the app-scope guard, the
//! handler's own gate and the facts loader — so the fresh read is kept on the
//! [`Caller`](crate::Caller) that asked, and on every clone of it. It lives
//! exactly as long as that caller does: one request.
//!
//! Sharing follows the value. A `Caller` built once and left in the request's
//! extensions (`Caller::from_extensions` hands out clones of it) shares one
//! read across every guard; a `Caller` built separately reads again. Either
//! way no read is older than the request it serves.

use std::fmt;
use std::sync::{Arc, OnceLock};

use oxy_authz::PlatformStanding as Grant;

/// The slot. Empty until the first read; never emptied.
#[derive(Clone, Default)]
pub(crate) struct GrantMemo(Arc<OnceLock<Option<Grant>>>);

impl GrantMemo {
    /// The grant this caller's request already read, if it has.
    pub(crate) fn get(&self) -> Option<Option<Grant>> {
        self.0.get().cloned()
    }

    /// Keep `grant` for the rest of the request. The first write wins; two
    /// guards racing read the same row and would store the same answer.
    pub(crate) fn set(&self, grant: Option<Grant>) {
        let _ = self.0.set(grant);
    }
}

/// A memo is not part of who a caller is: two callers that are the same user
/// on the same credential are equal whatever either has read so far.
impl PartialEq for GrantMemo {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for GrantMemo {}

impl fmt::Debug for GrantMemo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0.get() {
            Some(_) => "GrantMemo(read)",
            None => "GrantMemo(unread)",
        })
    }
}

#[cfg(test)]
mod tests {
    use oxy_authz::{PlatformRole, Scope};

    use super::*;

    fn grant() -> Grant {
        Grant::from_role(PlatformRole::AppOperator, Scope::All)
    }

    /// A clone shares the slot — that is the per-request memo — and a fresh
    /// memo starts empty.
    #[test]
    fn a_clone_shares_what_was_read_and_a_new_memo_has_read_nothing() {
        let memo = GrantMemo::default();
        assert_eq!(memo.get(), None);
        let shared = memo.clone();
        shared.set(Some(grant()));
        assert_eq!(memo.get(), Some(Some(grant())));
        assert_eq!(GrantMemo::default().get(), None);
    }

    /// "Read, and holds no grant" is kept too, and the first read stands.
    #[test]
    fn no_grant_is_remembered_and_the_first_read_wins() {
        let memo = GrantMemo::default();
        memo.set(None);
        assert_eq!(memo.get(), Some(None));
        memo.set(Some(grant()));
        assert_eq!(memo.get(), Some(None));
    }

    #[test]
    fn a_memo_never_makes_two_callers_differ() {
        let (read, unread) = (GrantMemo::default(), GrantMemo::default());
        read.set(Some(grant()));
        assert_eq!(read, unread);
    }
}
