//! Chat — channels, messages and live delivery.
//!
//! # Built, not bought
//!
//! The competitor this is measured against runs CometChat, and the sizing said
//! integrate. The call went the other way, so this is shaped to keep owning it
//! cheap: three tables, no bespoke realtime infrastructure, and fan-out over the
//! Postgres `LISTEN`/`NOTIFY` the task router already proves works here.
//!
//! # Two gates, and why they are different mechanisms
//!
//! * **May you reach this org's chat at all** — `oxy-authz`, `Ring::MemberStrict`.
//!   Strict on purpose: chat is the strongest case in the product for excluding
//!   the cross-tenant operator override. A staff member reading a tenant's
//!   conversations because they hold a global grant is not a feature.
//! * **Which channels within it** — a JOIN against `chat_channel_members`.
//!   A user's channel set is unbounded, so loading it into `PrincipalFacts` on
//!   every request would put an unbounded read on the hot path to answer what a
//!   `WHERE` clause answers for free.
//!
//! Membership is therefore never checked in a branch a handler could forget: it
//! is in the query that produces the rows. A handler that returns messages
//! without joining it returns nothing, rather than returning everything.
//!
//! # Fleet role
//!
//! Every route here is FleetOk (`route_roles` in this crate; `route_fleet` in
//! `oxy-app` before 2026-11-16), **including the SSE stream**. The
//! route-classification skill pins live streams to the ide, but that rule is
//! about runs executing in-process against a working copy. This stream is a
//! fan-out over persisted data, and pinning it to the singleton would mean chat
//! dies on every deploy and only works for whoever happens to be routed there.

pub mod delivery;
pub mod dto;
pub mod handlers;
