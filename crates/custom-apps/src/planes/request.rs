//! Choosing planes: what a write to an environment's `planes` column is
//! turned into before it is stored, and checked against
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! Every writer (`POST …/environments`, `PUT …/environments/{name}/planes`)
//! goes through [`PlaneSpec::from_request`] and then [`PlaneSpec::validate`].
//! The first turns what the request said into a spec and the object to store;
//! the second asks the facts a mode depends on, which are not in the request.

use chrono::{DateTime, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::env_branches;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use serde_json::{Map, Value};

use super::spec::{OltpMode, PRESET_FULL, PlaneSpec};
use super::{Plane, validate_fetch};

/// A `planes` choice that cannot be stored. The message names what is wrong;
/// the route answers it `400 invalid_planes`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PlaneError(pub String);

/// The facts [`PlaneSpec::validate`] needs, loaded once for a write by
/// [`PlaneContext::load`]. A phase whose mode depends on a fact adds the field
/// here, and the query to `load`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneContext {
    /// The environment the choice is for.
    pub environment: AppEnvironment,
    /// The `semantic` branch the write names, **when the workspace was asked
    /// and has it** ([`PlaneContext::with_existing_branch`]). Not a fact
    /// `load` can read (the lookup is `oxy-app`'s, through
    /// `oxy_app_core::semantic_pin_seam`), so the route asks the port and
    /// records the answer here. Left `None`, a spec that names a branch is
    /// refused: an unchecked branch is never accepted by default.
    pub existing_semantic_branch: Option<String>,
    /// The point it was or is being cut at: the create's `at`, else the
    /// stored row's `fork_at`. A `snapshot` Airhouse plane first reads there.
    pub fork_at: Option<DateTime<Utc>>,
    /// The fork point the write itself names (a create's `--at`), if any; a
    /// write to an existing environment names none, so an old stored fork
    /// point never refuses an unrelated change of planes.
    pub requested_at: Option<DateTime<Utc>>,
    /// The instant the write is judged at.
    pub now: DateTime<Utc>,
    /// Whether the app's org has an OLTP tenant: without one there is nothing
    /// to branch.
    pub org_has_oltp: bool,
    /// How many OLTP branches the org's other environments hold, in every
    /// status; this environment's own row is not counted, so keeping a branch
    /// it already has is never refused by the cap.
    pub other_oltp_branches: u64,
    /// The most OLTP branches one org may hold (`OXY_APP_ENV_OLTP_BRANCHES_MAX`).
    pub max_oltp_branches: u64,
    /// Why the choice's warehouse overrides cannot be stored, when the caller
    /// that can read the workspace's databases and secrets found one: a mapped
    /// database that is production's under another name, or that cannot be
    /// checked yet. [`PlaneContext::load`] leaves it `None`; the surface that
    /// owns the check (`custom_apps_nonproduction::check_overrides`) sets it
    /// before it validates a choice that has overrides.
    pub warehouse_refusal: Option<String>,
}

impl PlaneContext {
    /// The context of a write to `environment` of `app`: its stored row's
    /// fork point, if it has a row, and the org's OLTP branch facts. A create
    /// states its own fork point with [`Self::with_fork_at`].
    pub async fn load(
        db: &DatabaseConnection,
        app: &entity::apps::Model,
        environment: &AppEnvironment,
    ) -> Result<PlaneContext, DbErr> {
        let row = app_environments::Entity::find_by_id((app.id, environment.name()))
            .one(db)
            .await?;
        let own = env_branches::find_for_app(db, app.id, &environment.name()).await?;
        let org_branches = env_branches::count_for_org(db, app.org_id).await?;
        Ok(PlaneContext {
            environment: environment.clone(),
            existing_semantic_branch: None,
            fork_at: row
                .and_then(|row| row.fork_at)
                .map(|at| at.with_timezone(&Utc)),
            requested_at: None,
            now: Utc::now(),
            org_has_oltp: env_branches::has_tenant(db, app.org_id).await?,
            other_oltp_branches: org_branches.saturating_sub(u64::from(own.is_some())),
            max_oltp_branches: env_branches::max_per_org(),
            warehouse_refusal: None,
        })
    }

    /// Record that the workspace has `branch`, which the write's `semantic`
    /// choice may then name.
    pub fn with_existing_branch(mut self, branch: &str) -> PlaneContext {
        self.existing_semantic_branch = Some(branch.to_string());
        self
    }

    /// The same context, cut at `fork_at` — a create's `at`, which no row
    /// holds yet.
    pub fn with_fork_at(mut self, fork_at: Option<DateTime<Utc>>) -> Self {
        if fork_at.is_some() {
            self.fork_at = fork_at;
            self.requested_at = fork_at;
        }
        self
    }
}

/// A request's planes, expanded: the spec it means and the object that is
/// stored for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedPlanes {
    pub spec: PlaneSpec,
    /// The preset's entries with the explicit keys over them, as sent: what
    /// `app_environments.planes` holds. A plane left out takes its default.
    pub stored: Value,
}

impl PlaneSpec {
    /// Expand a request's `preset` (`"light"` or `"full"`) and `planes`: the
    /// preset's entries first (`PRESET_FULL` for `full`, none for `light`),
    /// then the explicit keys over them, then parse the result strictly. An
    /// unknown preset, a `planes` that is not an object, an unknown plane and
    /// an unknown mode are each an error naming it.
    pub fn from_request(
        preset: Option<&str>,
        planes: Option<&Value>,
    ) -> Result<RequestedPlanes, PlaneError> {
        expand(preset_entries(preset)?, planes)
    }

    /// Whether this choice may be stored for `ctx.environment`: the facts a
    /// mode depends on. One exhaustive destructure, one binding per plane, so
    /// a plane added to [`PlaneSpec`] does not compile until it has a line
    /// here. A plane with nothing to check yet is bound to `_`; a phase whose
    /// mode needs a check binds it and checks it.
    pub fn validate(&self, ctx: &PlaneContext) -> Result<(), PlaneError> {
        let PlaneSpec {
            build: _,
            semantic: _,
            reads: _,
            oltp,
            airhouse,
            warehouse,
            storage: _,
            secrets: _,
            messages: _,
            fetch,
            schedules: _,
            workspace_actions: _,
        } = self;
        // `semantic` is destructured above only to keep the list exhaustive;
        // its one check reads the branch the choice names.
        if let Some(branch) = self.semantic_branch()
            && ctx.existing_semantic_branch.as_deref() != Some(branch)
        {
            return Err(PlaneError(format!(
                "semantic: branch {branch:?} was not found in this app's workspace: push it, or \
                 choose another branch"
            )));
        }
        validate_oltp(*oltp, ctx)?;
        if let Some(mode) = airhouse {
            super::airhouse::check(*mode, &ctx.environment, ctx.requested_at, ctx.now)
                .map_err(PlaneError)?;
        }
        if let Some(setting) = fetch {
            validate_fetch(setting).map_err(PlaneError)?;
        }
        if let Some(warehouse) = warehouse {
            warehouse.check()?;
            if let Some(why) = ctx
                .warehouse_refusal
                .as_ref()
                .filter(|_| !warehouse.destinations.is_empty())
            {
                return Err(PlaneError(why.clone()));
            }
        }
        Ok(())
    }
}

/// `planes.oltp`: `branch` and `empty` are a sandbox's own Neon branch, which
/// staging never has, the org must have a database to cut, the org may hold
/// only so many, and a fork point must be one the database still has.
fn validate_oltp(oltp: Option<OltpMode>, ctx: &PlaneContext) -> Result<(), PlaneError> {
    if !matches!(oltp, Some(OltpMode::Branch | OltpMode::Empty)) {
        return Ok(());
    }
    if ctx.environment == AppEnvironment::Staging {
        return Err(PlaneError(
            "staging uses the org's OLTP staging branch; oltp modes branch and empty are for \
             sandboxes"
                .to_string(),
        ));
    }
    if !ctx.org_has_oltp {
        return Err(PlaneError(
            "this org has no OLTP database to branch; oltp modes branch and empty need one \
             (use --plane oltp=schema)"
                .to_string(),
        ));
    }
    if ctx.other_oltp_branches >= ctx.max_oltp_branches {
        return Err(PlaneError(env_branches::cap_refusal(
            ctx.other_oltp_branches,
            ctx.max_oltp_branches,
        )));
    }
    match ctx.requested_at {
        Some(at) => env_branches::check_fork_at(at, ctx.now).map_err(PlaneError),
        None => Ok(()),
    }
}

/// The entries `preset` expands to.
fn preset_entries(preset: Option<&str>) -> Result<&'static [(Plane, &'static str)], PlaneError> {
    match preset {
        None | Some("light") => Ok(&[]),
        Some("full") => Ok(PRESET_FULL),
        Some(other) => Err(PlaneError(format!(
            "unknown preset {other:?}: use \"light\" or \"full\""
        ))),
    }
}

/// [`PlaneSpec::from_request`] over a given preset table, so a test can state
/// one independent of `PRESET_FULL`.
fn expand(
    entries: &[(Plane, &str)],
    planes: Option<&Value>,
) -> Result<RequestedPlanes, PlaneError> {
    let mut stored = Map::new();
    for (plane, mode) in entries {
        stored.insert(plane.key().to_string(), Value::String((*mode).to_string()));
    }
    match planes {
        None | Some(Value::Null) => {}
        Some(Value::Object(explicit)) => stored.extend(explicit.clone()),
        Some(_) => {
            return Err(PlaneError(
                "planes must be an object of plane key to mode, like {\"storage\": \"branch\"}"
                    .to_string(),
            ));
        }
    }
    let stored = Value::Object(stored);
    let spec = PlaneSpec::parse(&stored).map_err(PlaneError)?;
    Ok(RequestedPlanes { spec, stored })
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
