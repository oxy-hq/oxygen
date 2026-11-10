//! Policy engine prototype — the Rust ↔ TypeScript boundary (Phase 1) and the
//! org-OLTP policy store behind it (Phase 2).
//!
//! The question this phase answers: can Rust boot a TypeScript policy script,
//! hand it data, and get a decision back — through ops alone, without leaking
//! memory across decisions or paying for serialization it does not need?
//!
//! ```text
//!   Rust host (PolicyEngine)                 js/policy.js (from ts/policy.ts)
//!   ────────────────────────                 ────────────────────────────────
//!   decide(id, "SKU-1") ── invoke ────────►  decide(sku, policyId)
//!                                              await OxyContext.getPolicy(policyId)
//!   op_oxy_get_policy (async) ◄─ policy id ──    │
//!     PolicySource read ── policy JSON ──────►   ▼
//!                                              OxyMath.hashVariant(sku) → variant
//!   op_oxy_write_decision ◄── decision JSON ── OxyAudit.record(decision)
//!     mock audit write (AuditLog)                │
//!   ◄─────────────────── returned decision ──────┘
//! ```
//!
//! Every value crosses as a JSON string, both ways, so the boundary has one
//! encoding and a malformed value is refused at the edge rather than deep in a
//! handler. The read goes to a [`PolicySource`]: the `policy_overrides` table in
//! the org's OLTP database (`ctx.oltp`, Neon), in an app writer's schema and
//! reached as that writer ([`OltpWriter`]), or a literal when no database is
//! configured. The write is still a mock of a `policy_decisions` row.
//!
//! [`sync`] copies `policy_overrides` to a local Parquet file through DuckDB,
//! reading a snapshot that does not lock writers out.
//!
//! The policy is written in TypeScript and compiled to JavaScript ahead of time
//! by `build-js.sh` (esbuild), the way `oxyc publish` compiles an Oxy Function,
//! so the engine itself only ever loads JavaScript.
//!
//! One [`PolicyEngine`] owns one V8 isolate and serves many decisions from it;
//! `tests` measures that the isolate's heap does not grow with the number of
//! decisions it has served.

mod engine;
mod loader;
mod ops;
mod policy;
mod store;
pub mod sync;
mod v8_platform;

pub use engine::{EngineError, PolicyEngine};
pub use policy::{AuditLog, Bounds, Policy, PolicyDecision, Variant};
pub use store::{OltpWriter, POLICY_OVERRIDES_TABLE, PolicySource, StoreError};

#[cfg(test)]
mod terminate_gap_tests;
#[cfg(test)]
mod tests;
