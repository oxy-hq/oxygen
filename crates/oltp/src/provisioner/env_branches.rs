//! A custom-app environment's own OLTP branch: cut it, reset it, delete it.
//!
//! Beside `branches.rs` (the org's one shared staging branch) and built the
//! same way: each operation holds the branch's advisory lock for its whole run
//! (`lock_env_branch`), every status move is compare-and-set, and the row is
//! unreachable to a resolver until its credentials are sealed. What differs is
//! that these are per app and per environment, disposable, and forked at a
//! point the caller picks.
//!
//! **Any error after the row exists marks it `failed`** with the error's text,
//! before the error is returned, so a half-built branch is never `ready`. The
//! next cut or reset resumes from `failed`. A cut returns the branch
//! `migrating`: applying the app's OLTP migrations and calling it `ready` is
//! the caller's step.
//!
//! Production is never a target. A cut whose provider answer names production
//! fails the row, and a reset or delete of a row that names it is refused
//! before any provider call.

use chrono::{DateTime, SubsecRound, Utc};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::entity::env_branches::{self as rows, EnvBranchMode, EnvBranchStatus};
use crate::entity::tenants as oltp_tenants;
use crate::env_branches::{
    EnvBranchKey, check_fork_at, expires_at, mark_env_branch, provider_name,
};
use crate::provider::{BranchInit, BranchRequest, ProjectBranch};

use super::branch_site::BranchSite;
use super::env_branch_meta::uncut;
use super::env_branches_state::CutTimes;
use super::{OltpProvisioner, ProvisionerError};

impl OltpProvisioner {
    /// Cut the environment's branch, or finish one a failed attempt left.
    ///
    /// `fork_at` is the instant of production to fork from (`None`: now).
    /// Idempotent: a branch already `ready` or `migrating` is returned
    /// untouched, whatever `mode` and `fork_at` this call names; a reset in
    /// flight is [`ProvisionerError::EnvBranchNotReady`].
    #[instrument(skip(self), fields(org_id = %key.org_id, app_id = %key.app_id, env = %key.env_name))]
    pub async fn cut_env_branch(
        &self,
        key: &EnvBranchKey,
        mode: EnvBranchMode,
        fork_at: Option<DateTime<Utc>>,
    ) -> Result<rows::Model, ProvisionerError> {
        check_fork(fork_at)?;
        let fork_at = fork_at.map(db_precision);
        let tenant = self.branchable_tenant(key.org_id).await?;
        let lock = self
            .lock_env_branch(tenant.id, key.app_id, &key.env_name)
            .await?;
        let out = self.cut_env_locked(&tenant, key, mode, fork_at).await;
        lock.release().await;
        let row = out?;
        info!(status = row.status.as_str(), "cut OLTP environment branch");
        Ok(row)
    }

    async fn cut_env_locked(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        mode: EnvBranchMode,
        fork_at: Option<DateTime<Utc>>,
    ) -> Result<rows::Model, ProvisionerError> {
        let existing = self.find_env_row(tenant, key.app_id, &key.env_name).await?;
        match existing.as_ref().map(|r| r.status) {
            Some(EnvBranchStatus::Ready | EnvBranchStatus::Migrating) => {
                return Ok(existing.expect("matched Some"));
            }
            Some(EnvBranchStatus::Resetting) => {
                let row = existing.expect("matched Some");
                return Err(uncut(tenant.org_id, &row));
            }
            _ => {}
        }
        let (existing, resumable) = self.resume_in_mode(tenant, existing, mode).await?;
        let (times, fork_at) = cut_plan(existing.as_ref().filter(|_| resumable), fork_at);
        let intent = self.upsert_cutting(tenant, key, mode, times, existing);
        let row = self.record_within_cap(tenant, key, intent).await?;
        let id = row.id;
        let out = self.run_cut(tenant, key, row, fork_at, times).await;
        self.failed_on_error(id, out).await
    }

    /// The row a cut resumes, and whether what it recorded is still the cut's
    /// to keep. A row recorded in another mode is never resumed: its branch
    /// is deleted first ([`Self::discard_other_mode`]), and the cut plans from
    /// nothing. A failed discard marks the row `failed`, still in its old mode,
    /// so the next attempt discards again.
    async fn resume_in_mode(
        &self,
        tenant: &oltp_tenants::Model,
        existing: Option<rows::Model>,
        mode: EnvBranchMode,
    ) -> Result<(Option<rows::Model>, bool), ProvisionerError> {
        match existing {
            Some(row) if row.mode != mode => {
                let id = row.id;
                let out = self.discard_other_mode(tenant, row).await;
                Ok((Some(self.failed_on_error(id, out).await?), false))
            }
            other => Ok((other, true)),
        }
    }

    async fn run_cut(
        &self,
        tenant: &oltp_tenants::Model,
        key: &EnvBranchKey,
        row: rows::Model,
        fork_at: Option<DateTime<Utc>>,
        times: CutTimes,
    ) -> Result<rows::Model, ProvisionerError> {
        // Read before the copy (`branch_ledger`).
        let snapshot = self
            .production_ledger_at(tenant.org_id, times.fork_at)
            .await?;
        let row_mode = row.mode;
        let req = env_request(tenant, key, row_mode, fork_at, times);
        let cut = self.provider.create_branch(&req).await?;
        let landing = Landing {
            tenant,
            key,
            cut,
            expected: EnvBranchStatus::Cutting,
            snapshot: &snapshot,
            times,
            mode: row_mode,
        };
        self.land_cut(row, landing).await
    }

    /// Mark `row_id` `failed` if `out` is an error, then hand `out` back. A
    /// row that changed under the operation is someone else's, and is left.
    pub(super) async fn failed_on_error<T>(
        &self,
        row_id: Uuid,
        out: Result<T, ProvisionerError>,
    ) -> Result<T, ProvisionerError> {
        let Err(e) = &out else { return out };
        if matches!(e, ProvisionerError::EnvBranchStateChanged { .. }) {
            return out;
        }
        let failed = mark_env_branch(
            &self.db,
            row_id,
            EnvBranchStatus::Failed,
            Some(e.to_string()),
        );
        if let Err(db_err) = failed.await {
            warn!("could not mark the environment branch failed: {db_err}");
        }
        out
    }

    /// Record the provider's answer, copy the ledger, mint the credentials,
    /// and leave the branch `migrating`.
    pub(super) async fn land_cut(
        &self,
        row: rows::Model,
        landing: Landing<'_>,
    ) -> Result<rows::Model, ProvisionerError> {
        let Landing {
            tenant,
            key,
            cut,
            expected,
            snapshot,
            times,
            mode,
        } = landing;
        if names_production_cut(tenant, &cut) {
            return Err(ProvisionerError::EnvBranchIsProduction {
                org_id: key.org_id,
                env_name: key.env_name.clone(),
                branch_id: cut.id,
            });
        }
        let row = self
            .record_env_cut(row, &cut, expected, times, mode)
            .await?;
        self.seed_branch_ledger(&cut.id, snapshot).await?;
        if row.mode == EnvBranchMode::Empty {
            self.copy_meta_ledger(tenant, &row, times.fork_at).await?;
        }
        let site = BranchSite::of_env(&row).ok_or_else(|| uncut(tenant.org_id, &row))?;
        // Roles before grants: a writer provisioned after the cut does not
        // exist on a Neon branch until its credential is minted there.
        self.mint_branch_credentials(tenant, &site).await?;
        self.converge_branch_writers(tenant, &site).await?;
        self.set_env_status(row, EnvBranchStatus::Migrating).await
    }
}

/// What a provider's answer is landed with: whose, the status the row must
/// still be in, and the ledger and fork point the copy was taken at.
pub(super) struct Landing<'a> {
    pub(super) tenant: &'a oltp_tenants::Model,
    pub(super) key: &'a EnvBranchKey,
    pub(super) cut: ProjectBranch,
    pub(super) expected: EnvBranchStatus,
    pub(super) snapshot: &'a [entity::custom_app_migrations::Model],
    pub(super) times: CutTimes,
    /// The mode the row records once landed: the cut's, or a reset's new one.
    pub(super) mode: EnvBranchMode,
}

/// The fork point and expiry a cut records, and the instant the provider is
/// asked to fork at.
///
/// A fresh cut — or one whose earlier attempt was discarded for a change of
/// mode — takes the caller's `fork_at`, else now. **A resumed one
/// (a `cutting` or `failed` row) keeps the row's own**, never re-stamped as
/// now: the branch a failed attempt left, or adopts by name, holds the data of
/// that earlier instant, and a ledger copied as of a later one would mark
/// migrations applied that the branch lacks. The one exception is a row the
/// provider never answered for, cut again with an explicit `fork_at` — nothing
/// of the old attempt exists, so the new instant is the intent.
fn cut_plan(
    existing: Option<&rows::Model>,
    fork_at: Option<DateTime<Utc>>,
) -> (CutTimes, Option<DateTime<Utc>>) {
    match existing {
        Some(row) if row.provider_branch_id.is_some() || fork_at.is_none() => {
            let kept = CutTimes {
                fork_at: row.fork_at.to_utc(),
                expires_at: row.expires_at.to_utc(),
            };
            (kept, Some(kept.fork_at))
        }
        _ => (cut_times(fork_at), fork_at),
    }
}

pub(super) fn check_fork(at: Option<DateTime<Utc>>) -> Result<(), ProvisionerError> {
    match at {
        Some(at) => check_fork_at(at, Utc::now()).map_err(ProvisionerError::ForkOutsideHistory),
        None => Ok(()),
    }
}

pub(super) fn cut_times(at: Option<DateTime<Utc>>) -> CutTimes {
    let fork_at = db_precision(at.unwrap_or_else(Utc::now));
    CutTimes {
        fork_at,
        expires_at: expires_at(fork_at),
    }
}

/// `at` as Postgres stores it: whole microseconds. A fork point is written to
/// the row and sent to the provider, and its expiry derived from it; taken at
/// a clock's full precision (nanoseconds on Linux) the provider would be told
/// an instant the row cannot record, so the two would disagree.
pub(super) fn db_precision(at: DateTime<Utc>) -> DateTime<Utc> {
    at.trunc_subsecs(6)
}

/// What the provider is asked for. An `empty` branch holds no production rows,
/// so the provider is told it never expires (`None`); the row still records
/// the 30 days it is reaped by (`expires_at` is not nullable).
pub(super) fn env_request(
    tenant: &oltp_tenants::Model,
    key: &EnvBranchKey,
    mode: EnvBranchMode,
    parent_timestamp: Option<DateTime<Utc>>,
    times: CutTimes,
) -> BranchRequest {
    let mut req = delete_request(tenant, key.app_id, &key.env_name, mode);
    req.parent_timestamp = parent_timestamp;
    if mode == EnvBranchMode::Branch {
        req.expires_at = Some(times.expires_at);
    }
    req
}

/// The request that names an environment's branch and nothing about when it
/// was cut: what a delete or a lookup by name needs. `init` follows the mode
/// because a provider recognises its own schema-only branch by it.
pub(super) fn delete_request(
    tenant: &oltp_tenants::Model,
    app_id: Uuid,
    env_name: &str,
    mode: EnvBranchMode,
) -> BranchRequest {
    let mut req = BranchRequest::staging(
        tenant.project_id.as_str(),
        tenant.branch_id.as_str(),
        provider_name(app_id, env_name),
        tenant.database_name.as_str(),
        tenant.owner_role.as_str(),
    );
    if mode == EnvBranchMode::Empty {
        req.init = BranchInit::SchemaOnly;
    }
    req
}

/// [`refuse_env_production`] for a branch known only by the id a provider
/// lookup returned.
pub(super) fn refuse_unrecorded_production(
    tenant: &oltp_tenants::Model,
    row: &rows::Model,
    found: &str,
) -> Result<(), ProvisionerError> {
    if crate::resolver::names_production(tenant, found, "", found) {
        return Err(ProvisionerError::EnvBranchIsProduction {
            org_id: tenant.org_id,
            env_name: row.env_name.clone(),
            branch_id: found.to_string(),
        });
    }
    Ok(())
}

/// Whether the provider's answer is production — by id, or by endpoint.
fn names_production_cut(tenant: &oltp_tenants::Model, cut: &ProjectBranch) -> bool {
    crate::resolver::names_production(tenant, &cut.id, &cut.host, &cut.database.name)
}

/// Refuse a row that names production, before any provider call acts on it.
pub(super) fn refuse_env_production(
    tenant: &oltp_tenants::Model,
    row: &rows::Model,
) -> Result<(), ProvisionerError> {
    let named = |s: &Option<String>| s.clone().unwrap_or_default();
    let (Some(id), host, database) = (
        row.provider_branch_id.as_deref(),
        named(&row.host),
        named(&row.database_name),
    ) else {
        return Ok(());
    };
    if crate::resolver::names_production(tenant, id, &host, &database) {
        return Err(ProvisionerError::EnvBranchIsProduction {
            org_id: tenant.org_id,
            env_name: row.env_name.clone(),
            branch_id: id.to_string(),
        });
    }
    Ok(())
}
