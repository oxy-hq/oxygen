//! Public webhook receivers. Mounted under the unauthenticated public
//! router because external services (Toast, Stripe, …) can't carry a user
//! JWT.

pub mod github;
pub mod toast;
