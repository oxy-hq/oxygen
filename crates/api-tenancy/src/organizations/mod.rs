//! Organization HTTP surface: org CRUD, member management (list / role
//! change / removal), and the invitation lifecycle (create / bulk / list /
//! revoke / accept), plus the invitation email sender.
//!
//! - [`dto`]: request/response serde types shared across handlers.
//! - [`ops`]: internal helpers — slug generation/reservation, invite-email
//!   normalization, per-org count queries, and the invitation email sender.
//! - [`org_handlers`] / [`member_handlers`] / [`invitation_handlers`]: the
//!   HTTP handler functions themselves, split by concern.

mod dto;
mod invitation_handlers;
mod member_handlers;
mod ops;
mod org_handlers;

pub use invitation_handlers::*;
pub use member_handlers::*;
// `pub` (not `pub(crate)`): these org helpers are reused by the extracted
// `oxy-api-partner-console` surface (invite + slug + org-creation flows).
pub use ops::{
    find_live_invitation, is_reserved_slug, normalize_invite_email, send_invitation_email,
    slugify_name, supersede_expired_invitations,
};
pub use org_handlers::*;
