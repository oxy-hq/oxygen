//! Policy engine prototype — Phase 1: the Rust ↔ TypeScript boundary.
//!
//! The question this phase answers: can Rust boot a TypeScript policy script,
//! hand it data, and get a decision back — through ops alone, without leaking
//! memory across decisions or paying for serialization it does not need?
//!
//! ```text
//!   Rust host (PolicyEngine)                 js/policy.js (from ts/policy.ts)
//!   ────────────────────────                 ────────────────────────────────
//!   decide("SKU-1") ─── invoke ───────────►  decide(sku)
//!                                              OxyContext.getPolicy("mock")
//!   op_oxy_get_policy  ◄── policy name ──────    │
//!     mock DB read ─────── policy JSON ──────►   ▼
//!                                              OxyMath.hashVariant(sku) → variant
//!   op_oxy_write_decision ◄── decision JSON ── OxyAudit.record(decision)
//!     mock audit write (AuditLog)                │
//!   ◄─────────────────── returned decision ──────┘
//! ```
//!
//! Every value crosses as a JSON string, both ways, so the boundary has one
//! encoding and a malformed value is refused at the edge rather than deep in a
//! handler. The two ops are mocks of what later phases make real: the read
//! becomes a `policy_overrides` lookup in Postgres (Phase 2), the write a
//! `policy_decisions` row.
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

pub use engine::{EngineError, PolicyEngine};
pub use policy::{AuditLog, PolicyDecision};

#[cfg(test)]
mod tests;
