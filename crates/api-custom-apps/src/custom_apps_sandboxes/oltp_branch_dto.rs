//! A sandbox's own OLTP branch as `env show` reports it
//! ([`OltpBranchDto`], `internal-docs/per-org-oltp-postgres.md` → Per-environment
//! branches): which mode it was cut in, whether `ctx.oltp` can use it yet, the
//! point it was forked at and when the provider reaps it.
//!
//! Read from the `oltp_env_branches` row, which the sandbox's own row cannot
//! hold: the cut runs on the worker fleet, so the row is the only record of how
//! far it has got. A sandbox on `schema` mode has no such row and reports
//! `null`; its schema is `oltp_schema`'s.

use chrono::{DateTime, Utc};
use oxy_oltp::entity::env_branches::{EnvBranchMode, Model};
use serde::Serialize;

/// One environment's OLTP branch. Every field is always present; one that does
/// not apply is `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OltpBranchDto {
    /// `branch` (a copy of production as of `fork_at`) | `empty` (its schema
    /// and none of its rows).
    pub mode: String,
    /// `cutting` | `migrating` | `ready` | `resetting` | `failed`. Only
    /// `ready` is served: any other is a `ctx.oltp` refusal that names its fix.
    pub status: String,
    /// The instant of production the branch was cut at.
    pub fork_at: DateTime<Utc>,
    /// When the provider reaps the branch, and the sandbox is torn down with
    /// it. `null` for `empty`: it holds no production rows, so the provider
    /// is told it never expires. The sandbox may still be deleted for age on
    /// another plane's copy: `EnvironmentDto::copies_expire_at` says when.
    pub expires_at: Option<DateTime<Utc>>,
    /// What failed, when `status` is `failed`.
    pub error: Option<String>,
    /// Whether a cut or reset is waiting or running on the queue. A queued one
    /// leaves `status` at `ready` until the worker takes it.
    pub recut_queued: bool,
}

impl OltpBranchDto {
    pub fn of(row: &Model) -> Self {
        let expires_at = match row.mode {
            EnvBranchMode::Branch => Some(row.expires_at.with_timezone(&Utc)),
            EnvBranchMode::Empty => None,
        };
        Self {
            mode: row.mode.as_str().to_string(),
            status: row.status.as_str().to_string(),
            fork_at: row.fork_at.with_timezone(&Utc),
            expires_at,
            error: row.error.clone(),
            recut_queued: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use oxy_oltp::entity::env_branches::EnvBranchStatus;
    use uuid::Uuid;

    use super::*;

    fn row(mode: EnvBranchMode, status: EnvBranchStatus, error: Option<&str>) -> Model {
        let fork = Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
        Model {
            id: Uuid::from_u128(1),
            tenant_row_id: Uuid::from_u128(2),
            app_id: Uuid::from_u128(3),
            env_name: "dev-a1".into(),
            mode,
            status,
            error: error.map(str::to_string),
            provider_branch_id: Some("br-1".into()),
            parent_branch_id: "br-0".into(),
            host: None,
            database_name: None,
            owner_role: None,
            owner_password_ciphertext: None,
            fork_at: fork.fixed_offset(),
            expires_at: (fork + Duration::days(30)).fixed_offset(),
            created_at: fork.fixed_offset(),
            last_reset_at: None,
            updated_at: fork.fixed_offset(),
        }
    }

    #[test]
    fn a_branch_reports_its_fork_point_and_the_day_it_is_reaped() {
        let dto = OltpBranchDto::of(&row(EnvBranchMode::Branch, EnvBranchStatus::Ready, None));
        assert_eq!(dto.mode, "branch");
        assert_eq!(dto.status, "ready");
        assert_eq!(
            dto.expires_at,
            Some(Utc.with_ymd_and_hms(2026, 12, 2, 9, 0, 0).unwrap())
        );
        assert_eq!(dto.error, None);
    }

    #[test]
    fn an_empty_branch_has_no_expiry_to_report() {
        let dto = OltpBranchDto::of(&row(EnvBranchMode::Empty, EnvBranchStatus::Migrating, None));
        assert_eq!(dto.mode, "empty");
        assert_eq!(dto.status, "migrating");
        assert_eq!(dto.expires_at, None, "the row's 30 days are not a promise");
    }

    #[test]
    fn a_failed_branch_carries_its_reason() {
        let dto = OltpBranchDto::of(&row(
            EnvBranchMode::Branch,
            EnvBranchStatus::Failed,
            Some("migration 0003 failed"),
        ));
        assert_eq!(dto.status, "failed");
        assert_eq!(dto.error.as_deref(), Some("migration 0003 failed"));
    }
}
