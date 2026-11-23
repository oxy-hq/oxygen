//! Fleet jobs: process-lifetime loops whose answer is global rather than per
//! replica — re-measure every app's asset silo, queue the usage backfills,
//! check the asset bucket's lifecycle rules, anchor the audit chain heads.
//!
//! They used to be [`BackgroundSeam`](super::BackgroundSeam)s gated on
//! `inproc_global_worker_enabled()`, which in a split fleet is true on exactly
//! one pod: the `ide` StatefulSet. Prod scaled that to zero, the serve replicas
//! answer false (`--no-workers`, no `OXY_INPROC_GLOBAL_WORKER`), and `oxy worker`
//! was never handed them — so they ran nowhere.
//!
//! Now they are started by every process that runs singleton work: each `oxy
//! worker` replica, and an `all`/`ide` `oxy serve`. The replicas have no leader,
//! so each job claims its interval in `fleet_job_claims`
//! ([`oxy_app_core::fleet_claim`]) and runs once per interval across them,
//! however many start it.

// The seam types are `oxy_app_core::seams`'s: a surface crate names them, and this
// module, which starts `oxy-app`'s own audit anchor, is not base.
use oxy_app_core::seams::{FleetJobDeps, FleetJobSeam};

/// `oxy-app`'s own fleet job: the hourly audit chain-head anchor (a no-op
/// without `OXY_AUDIT_ANCHOR_S3_BUCKET`).
const OWN_FLEET_JOBS: [FleetJobSeam; 1] = [FleetJobSeam {
    name: "audit_chain_anchor",
    start: start_audit_anchor,
}];

fn start_audit_anchor(deps: &FleetJobDeps<'_>) {
    crate::server::audit_anchor::spawn_audit_anchor_loop(deps.db.clone(), deps.shutdown.clone())
}

/// Start `oxy-app`'s fleet jobs and the surfaces' when this process runs
/// singleton work (`runs_singleton_work`, from `inproc_global_worker_enabled`:
/// true on `oxy worker` unless `OXY_INPROC_GLOBAL_WORKER=0`, and on an
/// `all`/`ide` `oxy serve`; false on a serve replica). Returns the names
/// started, in order.
pub fn start_fleet_jobs(
    surface_jobs: &[FleetJobSeam],
    deps: &FleetJobDeps<'_>,
    runs_singleton_work: bool,
) -> Vec<&'static str> {
    if !runs_singleton_work {
        return Vec::new();
    }
    let started: Vec<&'static str> = OWN_FLEET_JOBS
        .iter()
        .chain(surface_jobs)
        .map(|job| {
            (job.start)(deps);
            job.name
        })
        .collect();
    tracing::info!(jobs = ?started, "fleet jobs started; each claims its interval in fleet_job_claims");
    started
}

#[cfg(test)]
#[path = "fleet_jobs_tests.rs"]
mod tests;
