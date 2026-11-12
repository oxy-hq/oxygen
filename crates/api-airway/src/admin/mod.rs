//! `/api/admin/airway/*` — staff read/write of Airway's configuration: the
//! admission policy per source kind (`airway_source_config`, its global row
//! plus any per-workspace overrides) and the deployment-wide operational tier
//! (`airway_deployment_config`).
//!
//! Gated by `cap(Action::PlatformOperate)` — the capability the other
//! deployment-wide operational surfaces (`workspace_health` / `routing` /
//! `metrics`) already take — **not** strict `OXY_OWNER`. The capability is the
//! door; scope is the fence, and this surface carries its own because
//! `Resource::platform()` has no org for a guard to check. Every route that
//! returns or writes per-tenant rows fences: see [`handlers`]' module doc for
//! the two override writes and the listing, and [`preview`]'s for the scan.
//! `admin::mod`'s note at the mount point covers the one reach deliberately
//! left fleet-wide — the **global row**, which belongs to no org.
//!
//! That sentence used to point at the mount-point note for "the one reach left
//! fleet-wide" while [`preview`] was also unfenced, and unfenced over strictly
//! more than the listing returned: workspace ids, real `.airway.yml` paths,
//! resource names and parse errors from every tenant. The note covers a *row*,
//! not a route; a doc that reads as coverage is worse than none, so the claim
//! is now specific enough to be falsifiable.
//!
//! [`deployment`] is the third case that note has to cover, and it is the
//! **global row's** shape, not the listing's: a deployment-wide singleton that
//! belongs to no org, so there is nothing to fence it against. It carries no
//! per-tenant data at all.
//!
//! Stage 2 added the `airway_source_config` table and
//! `agentic_pipeline::airway_config::resolve_admission`, with no way to
//! edit those rows except SQL. Task 1 shipped the read side, Task 2 the
//! four write endpoints (upsert/delete, global and per-workspace), and
//! Task 3 the [`preview`] endpoint that says which resources a stricter
//! policy would reject before it is saved. The frontend (Tasks 4-6)
//! builds on all three.
//!
//! **Every route here is `FleetOk`** — Postgres-only, reads and writes
//! alike. That includes [`preview`], which reads workspace *content* but
//! reads it from the compile boundary (`airway_pipelines`, scoped to each
//! workspace's promoted revision) rather than the working copy, so it needs
//! no ide node. See [`preview`]'s module doc for why the compiled rows beat
//! the "but `airway_run` reads the working copy" argument. [`deployment`]'s
//! read also touches a process-local `OnceLock`, which is not node-local
//! *state* — see its module doc for why that is labelled in the payload
//! rather than fixed by pinning the route to the ide.
//!
//! Two tiers live under `/airway`, and they are not the same shape:
//!
//! - the **policy tier** — `contract_policy` and `environment`, per source
//!   kind with a per-workspace override, resolved on every run. That is
//!   [`handlers`] and [`preview`], over `airway_source_config`.
//! - the **operational tier** — the eight settings airway installs
//!   process-wide once at startup. That is [`deployment`], over the singleton
//!   `airway_deployment_config` row.
//!
//! The bar for both is the same and is the one airway's own plan sets: **a
//! knob must do something.** Every field on either tier is read by airway —
//! the operational eight are exactly `GlobalConfig`'s non-policy fields, and
//! `max_rewind` / `allow_unversioned_writes` / `partition_repull_budget` are
//! still absent from both because they have zero occurrences in airway's
//! `src/` and would be accepted, stored and ignored.
//!
//! `cursor_lag_floor` was on that absent list until airway 0.1.24, which is
//! the bar working rather than the bar moving: oxy contributed the key
//! upstream (#111), upstream grew the reader
//! (`global::resolve_cursor_lookback`), and only then did the column arrive.
//! The order is the point — a knob earns its column by having a consumer,
//! never the other way round.

pub(crate) mod deployment;
pub(crate) mod handlers;
pub(crate) mod preview;
pub(crate) mod preview_scan;

/// Every source kind the admin surface shows a card for, even with no
/// config row yet — matches `AirwayPipelineSpec::source.kind` spellings
/// (`crates/agentic/airway/src/config.rs`). Task 3's preview and Task 5's
/// frontend cards must read this same list so the surface can't drift
/// kind-by-kind. The write handlers also reuse this to reject a typo'd
/// `source_kind`: a row under an unknown kind would silently never appear
/// in [`handlers::get_config`]'s response, since that groups strictly by
/// this list.
pub(crate) const KNOWN_SOURCE_KINDS: &[&str] = &[
    "toast",
    "quickbooks",
    "weather",
    "rest_api",
    "netsuite",
    "sp_api",
];
