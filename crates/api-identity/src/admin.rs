//! The staff-console sections, mounted under `/api/admin` through
//! `SurfaceSeams::admin` ([`crate::admin_sections`]).
//!
//! - [`sandbox_agent_tokens`]: every staff member's sandbox agent tokens,
//!   behind `PlatformOperate`;
//! - [`standing_tokens`]: every personal token carrying `platform` or
//!   `partner` standing, behind `PlatformGrants`;
//! - [`users_admin`]: the cross-tenant user directory (`/users*`), behind
//!   `PlatformUsers`, with [`user_scope`], how a bounded grant reaches a
//!   person. Moved from `oxy-app`'s `admin::{users_admin, user_scope}` in the
//!   admin split (PR 5); users are Identity.
//!
//! `oxy-app` applies each section's capability guard, and the console's outer
//! staff guard and acting block, exactly as it did when these were in-tree
//! sections. The token sections' handlers live with the rest of the token
//! code (`user_tokens::{sandbox_staff, standing_staff}`).

pub mod sandbox_agent_tokens;
pub mod standing_tokens;
mod user_scope;
pub mod users_admin;
