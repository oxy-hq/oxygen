//! Where a `ctx.warehouse` / `ctx.tx` write lands outside production: the
//! database the build's `nonProduction.destinations` maps the named one to
//! (environments design §4.2; `env_policy::homes`).
//!
//! The order is the design's: production's destination gate on the database
//! the call names runs first (the caller's `check_write_destination`), then
//! the policy, then the **same gate on the mapped name** — declared in the
//! function's `destinations` and, unless it is Airhouse, named in
//! `customerWarehouseWrites` with a reason — then the publish check again, on
//! the config as it is now: not the same name, not the workspace's own Airhouse,
//! not production's host and user — nor any other production database the
//! mapping names — and refused when a secret behind either does not resolve.
//! All before any connector, and so any credential, exists. A database the
//! build does not map is held.
//!
//! Then every statement sent on the mapped connection passes the destination
//! fence (`env_policy::destination_sql`): a name that reaches production's
//! database, or a write qualified by any database but the mapped one's, is
//! held unsent.

use super::super::env_policy::destination_sql::{
    DestinationFence, admit_mapped_statement, mapped_statement_suffix,
};
use super::super::env_policy::homes::{self, Routed};
use super::super::env_policy::{self, HostOp};
use super::env_guard::HeldTarget;
use super::*;
use crate::server::api::custom_apps_nonproduction::{
    Unresolved, chain_refusal, configured_names, identity,
};

/// A write isolated to a mapped destination: what its write targets may, and
/// may not, be qualified by.
#[derive(Clone, Debug)]
pub(super) struct MappedWrite {
    mapped: Vec<String>,
    production: Vec<String>,
}

impl MappedWrite {
    /// The fence a statement on the mapped connector (`dialect`) passes.
    pub(super) fn fence(&self, dialect: SqlDialect) -> DestinationFence {
        DestinationFence {
            dialect,
            mapped: self.mapped.clone(),
            production: self.production.clone(),
        }
    }
}

impl ProjectFunctionHost {
    /// The database a write naming `database` runs against, and the home it
    /// was isolated to (`None`: as asked). A held or refused write is noted in
    /// the held-write log and its error returned.
    pub(super) async fn write_database(
        &self,
        op: HostOp,
        database: &str,
        surface: WriteSurface,
        target: HeldTarget<'_>,
    ) -> Result<(String, Option<MappedWrite>), String> {
        match homes::warehouse_database(&self.policy, op, database) {
            Routed::AsAsked(database) => Ok((database, None)),
            Routed::Isolated(mapped) => {
                if let Err(refusal) = self.check_write_destination(&mapped, surface) {
                    self.note_held(op, target).await;
                    return Err(self.mapped_refusal(database, &mapped, refusal));
                }
                let write = self
                    .separate_from_production(op, database, &mapped, target)
                    .await?;
                Ok((mapped, Some(write)))
            }
            Routed::NotRun(env_policy::Decision::Hold) => {
                self.note_held(op, target).await;
                Err(homes::unmapped_message(
                    op,
                    self.policy.environment(),
                    database,
                ))
            }
            Routed::NotRun(decided) => Err(match self.admit_decided(op, decided, target).await {
                Err(refused) => refused,
                // A `NotRun` is never `Allow`; if it were, nothing runs.
                Ok(()) => env_policy::refused_message(
                    op,
                    self.policy.environment(),
                    env_policy::MISROUTED_FIX,
                ),
            }),
        }
    }

    /// Refuse — fail closed — a mapping that is a production database under
    /// another name, judged now against the workspace's current config and
    /// secrets: the config can change after the publish that checked it
    /// (`custom_apps_nonproduction::chain_refusal`, `Unresolved::Refuse`).
    /// Otherwise, the databases the mapped connection's statements may name.
    async fn separate_from_production(
        &self,
        op: HostOp,
        database: &str,
        mapped: &str,
        target: HeldTarget<'_>,
    ) -> Result<MappedWrite, String> {
        let workspace = self.proj_ctx.workspace_manager();
        let databases = workspace.config_manager.list_databases();
        let secrets = &workspace.secrets_manager;
        let refusal = chain_refusal(
            self.policy.destinations(),
            database,
            mapped,
            &databases,
            secrets,
            Unresolved::Refuse,
        )
        .await;
        let names = match refusal {
            Some(why) => Err(why),
            None => {
                let keys = self.policy.destinations().keys();
                fence_databases(&databases, keys, mapped, secrets).await
            }
        };
        match names {
            Ok(write) => Ok(write),
            Err(why) => {
                self.note_held(op, target).await;
                Err(env_policy::refused_message(
                    op,
                    self.policy.environment(),
                    &why,
                ))
            }
        }
    }

    /// Hold `sql` unsent when it reaches past the mapped database; it is
    /// listed in the held row under the mapped database `namespace`.
    pub(super) async fn admit_on_mapped(
        &self,
        op: HostOp,
        fence: &DestinationFence,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let Err(held) = admit_mapped_statement(fence, sql) else {
            return Ok(());
        };
        self.note_held(op, ("warehouse", namespace, &held.verb, &held.table))
            .await;
        Err(format!(
            "{}{}",
            env_policy::held_message(op, self.policy.environment()),
            mapped_statement_suffix(&held.why)
        ))
    }

    /// The mapped name failed production's gate: say which mapping sent the
    /// write there, so the author fixes the manifest rather than the call.
    fn mapped_refusal(&self, database: &str, mapped: &str, refusal: String) -> String {
        format!(
            "in the {environment} environment `{database}` is written as `{mapped}` \
             (oxy-app.json nonProduction.destinations), and the mapped database must pass the \
             same check: {refusal}",
            environment = self.policy.environment(),
        )
    }
}

/// What the statement fence allows a write target to be qualified by: the
/// mapped entry's configured database (BigQuery: datasets), and never any
/// production database the mapping names — every key, not only the one this
/// write named. A production name equal to the mapped one is exempt only on
/// another host, where it is a different database. A database read from a
/// secret that does not resolve refuses the write.
async fn fence_databases<'a>(
    databases: &[oxy::config::model::Database],
    production_keys: impl Iterator<Item = &'a String>,
    mapped: &str,
    secrets: &oxy::adapters::secrets::SecretsManager,
) -> Result<MappedWrite, String> {
    let find = |name: &str| {
        databases
            .iter()
            .find(|db| db.name == name)
            .ok_or_else(|| format!("`{name}` is not configured for this project"))
    };
    let staging = find(mapped)?;
    let mapped_names = configured_names(staging, secrets).await?;
    let staging_host = identity(staging, secrets).await.host;
    let mut production = Vec::new();
    for key in production_keys {
        let entry = find(key)?;
        let other_host = identity(entry, secrets).await.host != staging_host;
        for name in configured_names(entry, secrets).await? {
            let same_as_mapped = mapped_names.iter().any(|m| m.eq_ignore_ascii_case(&name));
            if !(other_host && same_as_mapped) {
                production.push(name);
            }
        }
    }
    Ok(MappedWrite {
        mapped: mapped_names,
        production,
    })
}
