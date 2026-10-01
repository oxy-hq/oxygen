//! Where a write the policy decided lands — the pure half of the P5b homes,
//! shared by the host and the differential test so the test resolves each
//! write exactly as the host does.
//!
//! Two homes are decided by what the call names rather than by the op alone:
//!
//! - a `ctx.warehouse` / `ctx.tx` write lands in the database the build's
//!   `nonProduction.destinations` maps the named one to
//!   ([`warehouse_database`]); the host then runs the same destination gate on
//!   the mapped name before it connects;
//! - a `ctx.airhouse` write lands in the app schema's sibling
//!   ([`airhouse_write`]): checked as production would check it, moved, and
//!   checked again against the sibling (`airhouse::sql_retarget`).
//!
//! Neither ever answers production's resource for a non-production run: a
//! write with no home is [`Routed::NotRun`], which the host holds or refuses.
//!
//! The P5a homes are decided by the op alone: `ctx.storage` works in the
//! environment's silo ([`EnvPolicy::silo_for`]), `ctx.secrets.set` writes the
//! environment's secret path ([`EnvPolicy::secret_segment`]), and the `ctx.env`
//! overlay is carried here as data.

use std::collections::{BTreeMap, BTreeSet};

use airhouse::sql_retarget::check_retargeted;
use airhouse::sql_rules::{self, Access};

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

use super::{Decision, EnvPolicy, HostOp, MISROUTED_FIX, Target, held_message};
use crate::server::api::custom_apps_storage::Silo;

/// The P5b half of the policy: the homes a call's names pick, and the
/// decisions that depend on them. None turns an `Isolate` row into `Allow`;
/// a name with no home turns it into `Hold`.
impl EnvPolicy {
    /// The same policy with the running build's `nonProduction.destinations`.
    /// Ignored in production.
    pub fn with_destinations(mut self, destinations: BTreeMap<String, String>) -> Self {
        if !self.is_production() {
            self.destinations = destinations;
        }
        self
    }

    /// Where a non-production write to `database` lands: its mapping, when the
    /// build declares one that names a database no mapping starts from. `None`
    /// in production, for an unmapped database, for a mapping onto itself, and
    /// for a chain (`{a: b, b: c}` — `b` is a production database).
    pub fn mapped_destination(&self, database: &str) -> Option<&str> {
        self.destinations
            .get(database)
            .map(String::as_str)
            .filter(|mapped| !mapped.is_empty() && !self.destinations.contains_key(*mapped))
    }

    /// The running build's whole mapping — what the host re-checks a mapped
    /// write against (`custom_apps_nonproduction::chain_refusal`).
    pub fn destinations(&self) -> &BTreeMap<String, String> {
        &self.destinations
    }

    /// The sibling a non-production write to `app_schema` lands in,
    /// `app_<writer>__<env>`. `None` in production, and for a schema or an
    /// environment that cannot name one (`airhouse::app_schema`).
    pub fn sibling_schema(&self, app_schema: &str) -> Option<String> {
        if self.is_production() {
            return None;
        }
        airhouse::app_schema::environment_schema(app_schema, &self.environment.name())
    }

    /// [`Self::decide`] for a write naming `database`: a row isolated to a
    /// mapped destination holds instead when the build maps none for it.
    pub fn decide_on_database(&self, op: HostOp, database: &str) -> Decision {
        match self.decide(op) {
            Decision::Isolate(Target::MappedDestination)
                if self.mapped_destination(database).is_none() =>
            {
                Decision::Hold
            }
            decided => decided,
        }
    }

    /// [`Self::decide`] for a write to the app's own Airhouse schema: a row
    /// isolated to the sibling holds instead when no sibling can be named.
    pub fn decide_in_schema(&self, op: HostOp, app_schema: &str) -> Decision {
        match self.decide(op) {
            Decision::Isolate(Target::SiblingSchema)
                if self.sibling_schema(app_schema).is_none() =>
            {
                Decision::Hold
            }
            decided => decided,
        }
    }

    /// [`Self::decide`] for a statement, commit or rollback on a `ctx.tx`
    /// handle whose `begin` was isolated to `opened_into`: it runs on that
    /// handle's connection, which is already the isolated home. A handle
    /// opened as asked (`None`) is decided by the op alone.
    pub fn decide_on_handle(&self, op: HostOp, opened_into: Option<Target>) -> Decision {
        match (op, opened_into) {
            (
                HostOp::TxQuery | HostOp::TxExec | HostOp::TxCommit | HostOp::TxRollback,
                Some(target),
            ) if !self.is_production() => Decision::Isolate(target),
            _ => self.decide(op),
        }
    }
}

/// The P5a half of the policy: where an op isolated to the storage silo or
/// the secrets path works, and the `ctx.env` overlay carried as data — the
/// build's effective `shared` keys and the keys this run read through that
/// fallback. The host resolves its resources through these same functions
/// (`host::env_homes`), and so does the differential test.
impl EnvPolicy {
    /// The same policy with the manifest's `shared` keys. Ignored in
    /// production.
    pub fn with_shared_env(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        if !self.is_production() {
            self.shared_env = keys.into_iter().collect();
        }
        self
    }

    /// The same policy recording the keys this run's `ctx.env` read through
    /// the shared fallback. Ignored in production.
    pub fn with_read_through_fallback(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        if !self.is_production() {
            self.read_through_fallback = keys.into_iter().collect();
        }
        self
    }

    /// The keys whose `ctx.env` value may fall back to production's here.
    pub fn shared_env(&self) -> &BTreeSet<String> {
        &self.shared_env
    }

    /// Did this run's `ctx.env.<key>` come from production?
    pub fn read_through_fallback(&self, key: &str) -> bool {
        self.read_through_fallback.contains(key)
    }

    /// The silo an op isolated to [`Target::StorageSilo`] works in:
    /// `customer-app-storage/<app_id>~<env>/`.
    pub fn storage_silo(&self, app_id: Uuid) -> Silo {
        Silo::for_environment(app_id, &self.environment)
    }

    /// The secret-path segment an op isolated to [`Target::EnvSecrets`]
    /// writes under: `apps/<app_id>/<segment>/<KEY>`. `None` in production.
    pub fn secret_segment(&self) -> Option<String> {
        crate::server::api::custom_apps_secrets::scope::environment_segment(&self.environment)
    }

    /// The silo `op` of `app_id` works in here: production's when the op runs
    /// as asked, the environment's when isolated to [`Target::StorageSilo`];
    /// `None` for any other decision (held, refused, or another home), and
    /// for an isolated op of a production policy, which has no environment
    /// silo — its "own" would be production's.
    pub fn silo_for(&self, op: HostOp, app_id: Uuid) -> Option<Silo> {
        match self.decide(op) {
            Decision::Allow => Some(Silo::production(app_id)),
            Decision::Isolate(Target::StorageSilo) if !self.is_production() => {
                Some(self.storage_silo(app_id))
            }
            Decision::Isolate(_) | Decision::Hold | Decision::Refuse { .. } => None,
        }
    }
}

/// Where one write goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed<T> {
    /// Performed as asked, on the resource the call named: production, or a
    /// production-admitted run.
    AsAsked(T),
    /// Performed on the environment's isolated home.
    Isolated(T),
    /// Not performed; the decision says why (`Hold`, or `Refuse`).
    NotRun(Decision),
}

/// The statement an Airhouse write sends, and the schema it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirhouseWrite {
    pub schema: String,
    pub statement: String,
}

/// The database a warehouse write naming `database` lands in under `policy`.
pub fn warehouse_database(policy: &EnvPolicy, op: HostOp, database: &str) -> Routed<String> {
    match policy.decide_on_database(op, database) {
        Decision::Allow => Routed::AsAsked(database.to_string()),
        Decision::Isolate(Target::MappedDestination) => match policy.mapped_destination(database) {
            Some(mapped) => Routed::Isolated(mapped.to_string()),
            None => Routed::NotRun(Decision::Hold),
        },
        Decision::Isolate(_) => Routed::NotRun(misrouted()),
        decided => Routed::NotRun(decided),
    }
}

/// The statement an Airhouse write `sql` sends under `policy`, for an app
/// whose own schema is `app_schema`. `Err` is the refusal production would
/// give, raised before anything is decided — so a held write is refused for
/// exactly what production refuses.
pub fn airhouse_write(
    policy: &EnvPolicy,
    op: HostOp,
    sql: &str,
    app_schema: &str,
) -> Result<Routed<AirhouseWrite>, String> {
    let as_written = one_statement(
        sql_rules::check(sql, app_schema, Access::Write).map_err(|e| e.to_string())?,
    )?;
    Ok(match policy.decide_in_schema(op, app_schema) {
        Decision::Allow => Routed::AsAsked(AirhouseWrite {
            schema: app_schema.to_string(),
            statement: as_written,
        }),
        Decision::Isolate(Target::SiblingSchema) => {
            let Some(sibling) = policy.sibling_schema(app_schema) else {
                return Ok(Routed::NotRun(Decision::Hold));
            };
            let moved = check_retargeted(sql, app_schema, &sibling, Access::Write)
                .map_err(|e| e.to_string())?;
            Routed::Isolated(AirhouseWrite {
                schema: sibling,
                statement: one_statement(moved)?,
            })
        }
        Decision::Isolate(_) => Routed::NotRun(misrouted()),
        decided => Routed::NotRun(decided),
    })
}

/// The held error for a write to a database the build does not map: the
/// held message, plus the manifest key that would give it a staging home.
pub fn unmapped_message(op: HostOp, environment: &AppEnvironment, database: &str) -> String {
    format!(
        "{held} `{database}` has no {environment} copy: to write one instead, map it in \
         oxy-app.json — \"nonProduction\": {{ \"destinations\": {{ \"{database}\": \
         \"<{environment} database>\" }} }}.",
        held = held_message(op, environment),
    )
}

fn misrouted() -> Decision {
    Decision::Refuse { fix: MISROUTED_FIX }
}

fn one_statement(mut statements: Vec<String>) -> Result<String, String> {
    match statements.len() {
        1 => Ok(statements.remove(0)),
        n => Err(format!(
            "{n} statements in one call; send one statement per call"
        )),
    }
}

#[cfg(test)]
#[path = "homes_tests.rs"]
mod tests;
