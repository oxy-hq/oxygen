//! The Tenancy HTTP surface: who belongs to an org, and how orgs and their
//! workspaces come to exist.
//!
//! One bounded context, one crate (`internal-docs/domain-boundaries.md` S1).
//! It began as two sibling crates that split along the CALLER, not the
//! context — `oxy-api-onboarding` (an org admin creating a workspace) and
//! `oxy-api-partner-console` (a partner managing its client orgs) — which
//! left the org/workspace provisioning they share with nowhere to live but
//! `oxy-app`. Merged, that shared code has one owner, which is what lets the
//! rest of tenancy (`organizations`, the org-team handlers, the staff
//! console's org and workspace sections) move here next.
//!
//! Each module keeps its own routes and declarations; `oxy-server` mounts them
//! through the same seams as before.

pub mod onboarding;
pub mod partner_console;
