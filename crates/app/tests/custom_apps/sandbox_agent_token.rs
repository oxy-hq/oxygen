//! The sandbox agent token (`oxy_sbx_`) on the custom-app surfaces (sandbox
//! agent credential design §7.2), with a token minted through the real route
//! and presented to the routers production mounts.
//!
//! - `the_loop`: the whole sandbox loop, A1 to S3, and what each step records.
//! - `refusals`: everything outside the token's own sandbox, and that nothing
//!   moved.
//! - `refusal_shape`: each of those refusals is one JSON shape for the token,
//!   and the body the route always answered for its minter's other credentials.
//! - `second_refusal`: with the route allow-list gone, a channel publish, a
//!   production call and a production run are still refused.
//! - `second_refusal_writes`: so are promote, rollback, unpublish, delete and
//!   every other write to an app's production or staging state.
//! - `revocation`: the token stops on its next request when its minter loses
//!   the grant, is deactivated, or revokes it; a queued check is cancelled.
//! - `ownership`: what the token owns is decided on the row each write locks.
//! - `instance`: the token reads the sandbox it has now, never an earlier one
//!   that had its name.
//! - `token_ended`: the sweep tears a token's sandboxes down a day after it
//!   ended, and records it once.
//!
//! **Needs** Postgres only.

mod fixture;
mod instance;
mod ownership;
mod refusal_shape;
mod refusals;
mod revocation;
mod second_refusal;
mod second_refusal_writes;
mod the_loop;
mod token_ended;
