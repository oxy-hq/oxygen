//! Install airway's **deployment (operational) tier** at process boot.
//!
//! The tier itself — the singleton `airway_deployment_config` row, its mapping
//! onto airway's `GlobalConfig`, and the one-shot install — lives in
//! [`agentic_airway::deployment_config`]. This module is only the *seam*: when
//! in a process's life the install happens, and what a process with no
//! database does instead.
//!
//! # Why boot, and not the first run
//!
//! airway keeps `GlobalConfig` in a process-wide `OnceLock` that
//! `HttpConfig::default` and `RetryConfig::default` read, so **one install
//! covers every connector the process ever builds**. Installing at the top of
//! `run_pipeline` instead covers every airway *load* and nothing else — and at
//! least one site builds a source connector and talks to the vendor outside
//! any load: `agentic_pipeline::airway_run::discover_airway_source_tables`,
//! behind `POST /sources/discover`, the create-pipeline wizard's table picker.
//!
//! TLS is the sharp edge. An operator who configures a custom CA bundle would
//! find real runs honour it while the wizard's table picker fails to connect —
//! which reads as a broken source rather than a config gap, and sends them
//! looking at the vendor instead of at their own setting.
//!
//! Hoisting the install here closes that **without a signature change
//! anywhere**: no `DatabaseConnection` threaded into a discovery function
//! whose contract is explicitly "no DB/platform context required", and any
//! future connector site is covered by construction rather than by someone
//! remembering. It also matches the tier's documented semantics exactly — read
//! once, one-shot, restart to change.
//!
//! # The seams
//!
//! One per entry point that can build an airway source connector:
//!
//! | entry point | seam |
//! |---|---|
//! | `oxy serve`, and `oxy start` which delegates to it — every `OXY_ROLE` (`ide`, `serve`, `worker`, `all`) | `oxy-server`'s `commands::serve::start_server_and_web_app`, after migrations and before the router (and its in-process worker fleet) is built |
//! | `oxy worker` — the standalone fleet worker | `oxy-server`'s `commands::worker::run_worker`, after migrations and before `WorkerRuntime::start` |
//! | `oxy airway run` / `backfill` / `coverage` | `cli::commands::airway::connect_db`, the one connect path all three share |
//!
//! Deliberately absent, each for a reason that is checkable rather than a
//! guess:
//!
//! - **`oxy admin --airway`** seeds a `TaskSpec::Airway` onto the queue and
//!   exits. The connector is built by whichever worker claims the row, and
//!   that worker installed at its own boot.
//! - **`oxy compile`** and **`oxy run`** never build a source connector.
//!   `oxy run` in particular must keep working with no database at all, so
//!   giving it a boot install would trade a real capability for a setting it
//!   cannot use.
//!
//! # Boot never fails on this
//!
//! Two failure shapes, kept apart on purpose:
//!
//! - **No database.** [`install_deployment_tier`] takes `Option`, and `None`
//!   is a first-class answer: airway's own compiled-in timeout, retry and TLS
//!   settings stay in force, which is exactly right — there is no row to read.
//! - **A row airway refuses.** Logged at `warn` and boot continues. The tier's
//!   own rule is that a malformed row *fails the run* rather than falling back
//!   silently, and that rule is preserved by the `install_once` call still
//!   sitting in `run_pipeline`: `OnceCell::get_or_try_init` does not cache an
//!   `Err`, so the run re-reads the row and raises the error onto the
//!   operator's SSE stream, where it is legible. Refusing to *boot* over it
//!   would take down a whole serve replica for a setting most of it never
//!   touches.
//!
//! That surviving `run_pipeline` call is also the fallback for any process
//! with no oxy-app boot seam — an integration test, a future binary, an
//! embedder of `agentic-airway`. It is not a duplicate install: after a
//! successful boot install the `OnceCell` is populated, so the second call
//! returns without running its closure and without logging anything.

use agentic_airway::deployment_config;
use sea_orm::DatabaseConnection;

/// What the boot seam did, so a caller (and a test) can tell the three
/// outcomes apart instead of inferring them from log output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeploymentTierBoot {
    /// The row was read and airway's process-wide config resolved from it.
    /// Also the answer when there is no row: an absent row installs an
    /// all-`None` config, which is how `installed()` stays a reliable "this
    /// process resolved its tier" signal.
    Installed,
    /// This process has no database, so it has no tier to install. airway's
    /// built-in defaults stay in force.
    NoDatabase,
    /// The row could not be read, or airway refused it. Reported rather than
    /// raised — see the module doc.
    Unresolved(String),
}

/// Install the deployment tier for this process from `db`.
///
/// `None` means "this process has no database" — see the module doc. Never
/// returns an error and never panics: booting is not the place to litigate a
/// malformed row.
pub async fn install_deployment_tier(db: Option<&DatabaseConnection>) -> DeploymentTierBoot {
    let Some(db) = db else {
        tracing::debug!(
            "airway deployment tier: no database in this process, so nothing to \
             install — airway's built-in timeout, retry and TLS settings stay in force"
        );
        return DeploymentTierBoot::NoDatabase;
    };

    match deployment_config::install_once(db).await {
        Ok(()) => DeploymentTierBoot::Installed,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "airway deployment tier: could not install `{}` at boot — airway's \
                 built-in settings stay in force for every connector this process \
                 builds outside a run (source discovery, policy preview). An airway \
                 run will re-read the row and fail with this error, which is where \
                 an operator can see it.",
                deployment_config::TABLE
            );
            DeploymentTierBoot::Unresolved(e.to_string())
        }
    }
}

/// [`install_deployment_tier`] for a boot path that has not opened a
/// connection yet.
///
/// A connection failure is treated as "no database" rather than propagated:
/// every caller of this either already required a working database earlier in
/// boot (and so cannot reach here with one missing) or does not need the tier
/// badly enough to refuse to start over it.
pub async fn install_deployment_tier_from_env() -> DeploymentTierBoot {
    let db = oxy::database::client::establish_connection().await.ok();
    install_deployment_tier(db.as_ref()).await
}

#[cfg(test)]
#[path = "airway_boot_tests.rs"]
mod tests;
