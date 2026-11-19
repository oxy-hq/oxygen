//! When a pooled connection's credential is too close to expiry to hand out.
//!
//! # Why this exists, and why it must not be "simplified" away
//!
//! Oxy opens airhouse connections with ephemeral `eph_*` credentials minted at
//! [`airhouse::DEFAULT_INTERNAL_TTL`] — **900 seconds**. The pool, left to
//! itself, holds a connection for as long as it keeps being used, which is far
//! longer than that.
//!
//! When the credential expires, **airhouse closes the socket**. That is
//! deliberate on airhouse's side: returning a SQLSTATE does not evict
//! libpq-family drivers, so it drops the connection instead. The pool would
//! then learn about it the only other way available — `is_live()` going false
//! on a later checkout — and rebuild *at that moment*, on a request's critical
//! path, which means a fresh TCP connect + SCRAM + schema fetch through
//! airhouse's 30-second admission queue. Every busy identity does this every
//! ~15 minutes, and each one is admission pressure on a data plane that admits
//! only 32 connections per pod.
//!
//! So the rule is: **retire on our clock, not on airhouse's.** A connection
//! whose credential is within [`RETIRE_BUFFER_SECS`] of expiring is treated
//! exactly like a dead one — the slot is rebuilt with a freshly minted
//! credential, before airhouse has any reason to hang up.
//!
//! A future reader looking at this and the `is_live()` check together will see
//! two paths to the same rebuild and be tempted to keep only the liveness one,
//! since it does eventually catch an expired credential. It catches it *after*
//! the connection is already gone and a user is already waiting. That is the
//! whole difference, and it is the reason this module is here.

/// How far ahead of `expires_at` a connection stops being handed out.
///
/// The broker uses 60s for the same question about a *credential* — enough for
/// a slow caller to finish a SCRAM handshake ([`CACHE_REFRESH_BUFFER`] in
/// `airhouse::broker`). A pooled *connection* needs more, because handing it
/// out commits it to the whole query the next caller is about to run, not just
/// a handshake. 120s is that 60s plus room for a query that starts just before
/// the handoff; against a 900s TTL it gives up ~13% of each connection's life
/// to be sure no query dies mid-flight.
///
/// [`CACHE_REFRESH_BUFFER`]: airhouse::broker
pub(super) const RETIRE_BUFFER_SECS: u64 = 120;

/// True when `deadline` is near enough that the connection must not be handed
/// out again.
///
/// `None` means the credential does not expire (static airhouse, where the
/// password is configured rather than minted) — nothing to retire for.
pub(super) fn credential_spent(deadline: Option<u64>, now: u64, buffer_secs: u64) -> bool {
    match deadline {
        None => false,
        Some(at) => at <= now.saturating_add(buffer_secs),
    }
}

#[cfg(test)]
mod tests {
    use super::{RETIRE_BUFFER_SECS, credential_spent};

    const NOW: u64 = 1_000_000;

    #[test]
    fn a_credential_with_no_expiry_is_never_spent() {
        // Static airhouse: the password is configured, not minted, so there is
        // no deadline to retire against.
        assert!(!credential_spent(None, NOW, RETIRE_BUFFER_SECS));
    }

    #[test]
    fn a_fresh_credential_is_not_spent() {
        // Just minted at the 900s internal TTL.
        assert!(!credential_spent(Some(NOW + 900), NOW, RETIRE_BUFFER_SECS));
    }

    #[test]
    fn a_credential_is_retired_before_airhouse_would_close_the_socket() {
        // The point of the whole module: inside the buffer but NOT yet expired,
        // so airhouse would still accept it — and we retire anyway, rather than
        // rebuilding later on a request's critical path.
        let deadline = NOW + RETIRE_BUFFER_SECS - 1;
        assert!(deadline > NOW, "deadline is still in the future");
        assert!(credential_spent(Some(deadline), NOW, RETIRE_BUFFER_SECS));
    }

    #[test]
    fn the_buffer_boundary_counts_as_spent() {
        assert!(credential_spent(
            Some(NOW + RETIRE_BUFFER_SECS),
            NOW,
            RETIRE_BUFFER_SECS
        ));
        assert!(!credential_spent(
            Some(NOW + RETIRE_BUFFER_SECS + 1),
            NOW,
            RETIRE_BUFFER_SECS
        ));
    }

    #[test]
    fn an_already_expired_credential_is_spent() {
        assert!(credential_spent(Some(NOW - 1), NOW, RETIRE_BUFFER_SECS));
        assert!(credential_spent(Some(0), NOW, RETIRE_BUFFER_SECS));
    }

    #[test]
    fn a_clock_near_u64_max_saturates_instead_of_wrapping() {
        // `now + buffer` cannot be represented, so `saturating_add` pins it at
        // `u64::MAX` and the answer is "spent". That is both correct — a
        // deadline one second out really is inside a 120s buffer — and the safe
        // direction: a wrapping add would have reported "fresh" and handed out
        // a connection Airhouse was about to close.
        assert!(credential_spent(Some(u64::MAX), u64::MAX - 1, 120));
    }
}
