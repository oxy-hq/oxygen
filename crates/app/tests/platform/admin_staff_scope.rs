//! A platform grant bounded to named orgs sees and touches only those orgs — on
//! every `/admin/*` surface, not just the ones that remembered.
//!
//! Staff standing is a grant: a role (a capability preset) and a scope (every org,
//! or a list). The console's capability gates decide on `Resource::platform()`,
//! which has no org, so a grant bounded to org A passes every one of them; the
//! handler behind the gate is the only thing that can narrow what it returns. A
//! security review of other work read three handlers that did not —
//! `admin::audit::list_audit`, `admin::explorer` and `admin::internal_jobs` — and
//! auditing the rest of the console against the same question found the same gap on
//! the assume-session log, the org and workspace directories, LLM usage, workspace
//! health, the compile operator surface and one invitation write.
//!
//! Each case drives the real handler as three callers against a database of its
//! own: a `global_admin` **bounded to org A**, a `global_admin` with `scope_all`,
//! and the Global Owner. The rule being pinned (`admin::scope`):
//!
//! * a **listing** is narrowed in the query, ahead of the paging — a page is never
//!   short and a total never counts a row the caller cannot see;
//! * a row named **by id** outside the grant answers what a missing row answers;
//! * a row with **no org** is platform-level: all-orgs grants and the Global Owner
//!   only;
//! * an all-orgs grant and the Global Owner see exactly what they saw before.
//!
//! Every "bounded" case here was watched failing against the unfenced handlers
//! before the fix — they are the proof the leak was real, not a restatement of the
//! code. The "unbounded" cases are the control: they pass on both sides of it.
//!
//! Database-backed through [`crate::common::test_db_with`] (`Schema::All`: the task
//! queue and runs live in the runtime tables), so each test owns its database and
//! every total asserted here is exact.

mod audit_and_explorer;
mod compiles;
mod directories;
mod fences;
mod fixture;
mod internal_jobs;
