//! Whose call this is, for every key a route invocation is remembered under.
//!
//! Three things outlive one function call and are looked up by the next one:
//! the opt-in result cache, the idempotency record and the rate-limit bucket.
//! Each used to be keyed by some subset of (app, function, user), and none of
//! them by the app **environment** — which is a production isolation hole the
//! moment a second environment can call a function
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4):
//!
//! - **Result cache.** Staging and production serve the same build, so a
//!   `(build, function, user, body)` key hands a staging result to a production
//!   caller.
//! - **Idempotency.** A production call reusing a key already spent in staging
//!   replays staging's stored result and **skips the production write**.
//! - **Rate limit.** The dev loop spends the same person's production budget.
//!
//! So each of those keys is built from a [`CallScope`], and the environment is
//! a field of it rather than a parameter someone can forget to pass. Today
//! every caller is production; the scope is what keeps that true of the keys
//! once it stops being true of the callers.

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

/// One route call's identity, as the per-call keys see it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CallScope<'a> {
    pub app_id: Uuid,
    /// The environment the call ran in. Stored as
    /// `app_function_invocations.environment` and folded into every key.
    pub environment: &'a AppEnvironment,
    pub function_name: &'a str,
    pub user_id: Uuid,
    /// The sandbox agent token the call authenticated with, recorded on the
    /// invocation row. Never part of a key: the keys are the user's.
    pub credential_token_id: Option<Uuid>,
}

impl CallScope<'_> {
    /// The `app_function_invocations.environment` value, and the scope label
    /// the in-memory keys carry.
    pub(crate) fn environment_name(&self) -> String {
        self.environment.name()
    }
}
