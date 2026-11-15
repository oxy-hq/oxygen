//! The assignment graph's HTTP surface.
//!
//! One table, five product surfaces. Tasks, Site visits, Location launcher,
//! Training and Compliance all reduce to *somebody owes somebody a piece of
//! work at a place by a time*, and the two views every one of them needs are
//! the same two: **assigned to me** and **supervised by me**.
//!
//! # Authorization, and why the filter IS the gate
//!
//! Work-item reads are self-scoped: you see items you are the assignee of, the
//! supervisor of, or that are assigned to a role you hold at that location.
//! That is a query filter rather than an `oxy-authz` ring, for the same reason
//! chat channel membership is — the set is unbounded per user, so loading it
//! into `PrincipalFacts` would put an unbounded read on the hot path.
//!
//! It also has to work for someone who is **not an org member at all**. A
//! frontline worker enrolled by PIN holds no `org_members` row by design, and
//! the entire point of assigning them work is that they can see it. A gate
//! written as "is a member of the org" would have locked out precisely the
//! people the graph exists to route work to.
//!
//! Managing the shape of the org — creating locations, defining roles — is a
//! different authority and does go through the model: `Action::ManageLocations`
//! and `Action::ManageOrgRoles`, both on `Ring::OrgAdmin`.
//!
//! # Fleet role
//!
//! FleetOk throughout (declared in [`crate::route_roles`]): Postgres only, no
//! working copy. "Assigned to me" must survive a deploy, or every store loses
//! its checklist the moment somebody ships.

pub mod dto;
pub mod handlers;
