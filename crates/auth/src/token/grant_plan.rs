//! How an edit's `grants` replaces a token's grant set — the row-level plan,
//! decided with no database.
//!
//! "Replaces" has three edges, and each is a way to hand back more than the
//! owner may hold or to lose something that is not theirs to lose:
//!
//! - **A grant its org revoked stays, revoked.** It is the org's decision and
//!   the token's history: an edit neither deletes the row nor inserts a live
//!   twin beside it. Asking for the same target again is simply not granted.
//! - **An `app_publish` grant can be kept or dropped, never added.** The edit
//!   dialog sends back the ones the token holds; a new one is minted by the
//!   publish-token flow, which is where the authority to publish is checked.
//! - **An unchanged grant keeps its row**, so its id is stable across edits and
//!   the audit row's before/after names what actually changed.

use entity::api_token_grants::{self, KIND_APP_PUBLISH, KIND_WORKSPACE};
use uuid::Uuid;

use super::access::GrantWant;
use super::personal::GrantSpec;

/// What a grant row is a grant *on*.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Workspace {
        org_id: Uuid,
        workspace_id: Option<Uuid>,
    },
    AppPublish {
        app_id: Uuid,
    },
}

/// `None` for a row of a kind this release does not know, or an `app_publish`
/// row with no app: it matches nothing, so a replace drops it.
fn target_of(row: &api_token_grants::Model) -> Option<Target> {
    match row.kind.as_str() {
        KIND_WORKSPACE => Some(Target::Workspace {
            org_id: row.org_id,
            workspace_id: row.workspace_id,
        }),
        KIND_APP_PUBLISH => row.app_id.map(|app_id| Target::AppPublish { app_id }),
        _ => None,
    }
}

fn wanted_target(want: &GrantWant) -> Target {
    match want {
        GrantWant::Workspace {
            org_id,
            workspace_id,
            ..
        } => Target::Workspace {
            org_id: *org_id,
            workspace_id: *workspace_id,
        },
        GrantWant::AppPublish { app_id } => Target::AppPublish { app_id: *app_id },
    }
}

/// The writes that turn the stored grants into the wanted ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantPlan {
    /// Live rows to delete.
    pub delete: Vec<Uuid>,
    /// Workspace grants to insert.
    pub insert: Vec<GrantSpec>,
    /// Live rows left exactly as they are.
    pub kept: Vec<Uuid>,
}

impl GrantPlan {
    pub fn changes_nothing(&self) -> bool {
        self.delete.is_empty() && self.insert.is_empty()
    }

    /// How many live grants the token holds once the plan is applied.
    pub fn live_after(&self) -> usize {
        self.kept.len() + self.insert.len()
    }
}

/// An `app_publish` grant was asked for that the token does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotHeld {
    pub app_id: Uuid,
}

/// Replace the live grants among `existing` with `wanted`.
pub fn replace(
    existing: &[api_token_grants::Model],
    wanted: &[GrantWant],
) -> Result<GrantPlan, NotHeld> {
    let (revoked, live): (Vec<_>, Vec<_>) = existing.iter().partition(|g| g.revoked_at.is_some());
    // A revoked ORG-WIDE grant is the org ending the token's reach into it
    // altogether (the inventory's revoke-grant): nothing in that org is granted
    // again, on any target. A revoked workspace grant bars that workspace.
    let revoked_by_org = |target: Target| {
        revoked.iter().any(|g| match (target_of(g), target) {
            (
                Some(Target::Workspace {
                    org_id: blocked,
                    workspace_id: None,
                }),
                Target::Workspace { org_id, .. },
            ) => blocked == org_id,
            (held, _) => held == Some(target),
        })
    };

    let mut plan = GrantPlan::default();
    for want in wanted {
        let target = wanted_target(want);
        if revoked_by_org(target) {
            continue;
        }
        let held = live.iter().find(|g| target_of(g) == Some(target));
        match (want, held) {
            (GrantWant::AppPublish { .. }, Some(row)) => plan.kept.push(row.id),
            (GrantWant::AppPublish { app_id }, None) => return Err(NotHeld { app_id: *app_id }),
            (GrantWant::Workspace { ceiling, .. }, Some(row))
                if row.role_ceiling.as_deref() == Some(ceiling.as_str()) =>
            {
                plan.kept.push(row.id);
            }
            // New, or held at another ceiling: the old row (if any) is deleted
            // below, as every live row that was not kept is.
            (
                GrantWant::Workspace {
                    org_id,
                    workspace_id,
                    ceiling,
                },
                _,
            ) => plan.insert.push(GrantSpec {
                org_id: *org_id,
                workspace_id: *workspace_id,
                ceiling: *ceiling,
            }),
        }
    }
    plan.delete = live
        .iter()
        .map(|g| g.id)
        .filter(|id| !plan.kept.contains(id))
        .collect();
    Ok(plan)
}

/// Drop every live grant — the token became all-access. Revoked rows stay.
pub fn clear(existing: &[api_token_grants::Model]) -> GrantPlan {
    GrantPlan {
        delete: existing
            .iter()
            .filter(|g| g.revoked_at.is_none())
            .map(|g| g.id)
            .collect(),
        ..GrantPlan::default()
    }
}

#[cfg(test)]
#[path = "grant_plan_tests.rs"]
mod tests;
