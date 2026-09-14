//! The document model's HTTP surface.
//!
//! Knowledge base and Compliance are the same primitive seen twice: a folder
//! tree of documents, each with a visibility rule, some of them scoped to one
//! store and some carrying an expiry date.
//!
//! # Reads are a filter, writes are a ring
//!
//! The asymmetry is the whole design and it is worth stating before the code.
//!
//! **Who may read** is a query filter ([`visibility`]), not an `oxy-authz`
//! ring, because the reader is often a frontline worker who holds no
//! `org_members` row by design and because the readable set is unbounded per
//! user — a `PrincipalFacts` fact would put an unbounded read on every request.
//! That is the call `/work` and the notifications inbox already made, for the
//! same two reasons.
//!
//! **Who may write** is `Action::ManageDocuments` on `Ring::OrgAdmin`, taken
//! through the `OrgAdmin` extractor. The differential suite already pins that
//! pairing — a manage route mounted behind any other guard fails
//! `every_org_scoped_document_write_takes_the_orgadmin_extractor`, which scans
//! the handlers against the router rather than the model against an oracle.
//!
//! # Why the read routes are not nested under `/orgs/{org_id}`
//!
//! Nesting would put `org_middleware` in front, and `org_middleware` rejects
//! exactly the callers these routes exist for. So the standing check is made by
//! hand in [`visibility::resolve_standing`] instead of being absent because the
//! route moved — the same shape, and the same hazard, as `/work`.
//!
//! # Fleet role
//!
//! `route_fleet` everywhere except [`ask`]. Nothing else here reads a working
//! copy, `.git` or the state dir: Postgres for the rows, a presigned
//! object-store URL for the bytes. Reading the sanitiser SOP has to survive a
//! deploy.
//!
//! [`ask`] is the one exception and it is `route_ide`, because writing an
//! answer means resolving an agent config out of the workspace working copy.
//! It is a separate route rather than a flag on [`search`] for exactly that
//! reason: search must keep serving from every replica while the ide restarts,
//! and a shared route would have pinned every search in the product to the
//! singleton.

pub mod ask;
pub mod ask_agent;
pub mod ask_sessions;
pub mod categories;
pub mod dto;
pub mod handlers;
pub mod hydrate;
pub mod manage;
pub mod review;
pub mod search;
pub mod shelf;
pub mod storage;
pub mod versions;
pub mod visibility;
