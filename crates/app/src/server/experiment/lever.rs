//! One lever resolver, three callers (index C2b): pricing refuses a proposal
//! whose `via` does not resolve; registration (PR 4) refuses the registration;
//! dispatch (PR 4) fails the flip. Pricing and registration also refuse a lever
//! whose automation never reads the units it is sent (`lever_refs`).

use std::collections::HashMap;

use entity::{automation_definitions, workspaces};
use oxy_experiment::ExperimentConfig;
use oxy_experiment::run::LeverState;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use super::lever_refs;

/// The automation a lever name resolves to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLever {
    /// Workspace-relative path: what `TaskSpec::Automation { workflow_ref }` takes.
    pub file_path: String,
    /// The promoted revision it was found at.
    pub revision_id: Uuid,
}

/// Why a lever did not resolve. Each message states only what the lookup tested.
#[derive(Debug, thiserror::Error)]
pub enum LeverRefusal {
    #[error("the workspace has no promoted revision, so `{via}` resolves to no automation")]
    NoPromotedRevision { via: String },
    #[error("no automation named `{via}` exists in the promoted revision {revision}")]
    NotFound { via: String, revision: Uuid },
    #[error(
        "`{via}` names {} automations in the promoted revision {revision} ({}); a lever must resolve to exactly one",
        .paths.len(),
        .paths.join(", ")
    )]
    Ambiguous {
        via: String,
        revision: Uuid,
        paths: Vec<String>,
    },
    /// A database error: a fault, not a verdict on the lever.
    #[error("the workspace's automations could not be read: {0}")]
    Database(sea_orm::DbErr),
}

impl LeverRefusal {
    /// `true` only for [`LeverRefusal::Database`]: pricing settles that row `failed`.
    pub fn is_fault(&self) -> bool {
        matches!(self, Self::Database(_))
    }
}

/// The verdict on the file paths that carry `via` at `revision`.
pub fn choose(via: &str, revision: Uuid, paths: &[String]) -> Result<ResolvedLever, LeverRefusal> {
    match paths {
        [one] => Ok(ResolvedLever {
            file_path: one.clone(),
            revision_id: revision,
        }),
        [] => Err(LeverRefusal::NotFound {
            via: via.to_string(),
            revision,
        }),
        many => Err(LeverRefusal::Ambiguous {
            via: via.to_string(),
            revision,
            paths: many.to_vec(),
        }),
    }
}

/// Resolve `via` by name at the workspace's promoted revision.
pub async fn resolve_lever(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    via: &str,
) -> Result<ResolvedLever, LeverRefusal> {
    let head = workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(LeverRefusal::Database)?
        .and_then(|w| w.current_revision_id)
        .ok_or_else(|| LeverRefusal::NoPromotedRevision {
            via: via.to_string(),
        })?;
    let rows = automation_definitions::Entity::find()
        .filter(automation_definitions::Column::RevisionId.eq(head))
        .filter(automation_definitions::Column::Name.eq(via))
        .order_by_asc(automation_definitions::Column::FilePath)
        .all(db)
        .await
        .map_err(LeverRefusal::Database)?;
    let paths: Vec<String> = rows.into_iter().map(|r| r.file_path).collect();
    choose(via, head, &paths)
}

/// Pricing's and registration's lever check: both sides of the lever. `Ok(lines)`
/// has one `lever.on: ...` / `lever.off: ...` line per side that did not resolve,
/// or resolved to an automation that never reads the units it is sent
/// ([`super::lever_refs`]); empty when both are fine. `Err(reason)` is a
/// database fault.
pub async fn check_levers(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    cfg: &ExperimentConfig,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    // No lever: nothing to resolve. Registration refuses it on its own
    // (`is_registrable`); pricing and a retrospective need none.
    let Some(l) = &cfg.lever else {
        return Ok(lines);
    };
    for (side, via) in [("lever.on", &l.on.via), ("lever.off", &l.off.via)] {
        match resolve_lever(db, workspace_id, via).await {
            Ok(lever) => {
                let refusal = lever_refs::unit_reference_refusal(db, cfg, via, &lever)
                    .await
                    .map_err(|e| LeverRefusal::Database(e).to_string())?;
                lines.extend(refusal.map(|r| format!("{side}: {r}")));
            }
            Err(r) if r.is_fault() => return Err(r.to_string()),
            Err(r) => lines.push(format!("{side}: {r}")),
        }
    }
    Ok(lines)
}

/// [`check_levers`] for many designs in one request: a design whose lever
/// sides, unit grain and grouping match one already checked gets that verdict
/// without a query, so a list costs a lookup per distinct lever, not per
/// experiment. Lives for one request: a promote between two lists is seen.
#[derive(Default)]
pub struct LeverChecks {
    seen: HashMap<String, Result<Vec<String>, String>>,
}

impl LeverChecks {
    pub async fn check(
        &mut self,
        db: &impl ConnectionTrait,
        workspace_id: Uuid,
        cfg: &ExperimentConfig,
    ) -> Result<Vec<String>, String> {
        // Everything `check_levers` reads from the design.
        let Some(l) = &cfg.lever else {
            return Ok(Vec::new());
        };
        let key = format!(
            "{}\n{}\n{:?}\n{}",
            l.on.via,
            l.off.via,
            l.per,
            cfg.assign_by.is_some()
        );
        if let Some(seen) = self.seen.get(&key) {
            return seen.clone();
        }
        let verdict = check_levers(db, workspace_id, cfg).await;
        self.seen.insert(key, verdict.clone());
        verdict
    }
}

/// The runner's view of `resolve_lever`: a lever, a refusal to record (the
/// registration is refused, the flip fails), or a database error to retry on
/// the next tick — never a flip failed because the database blinked.
pub async fn resolve_for_runner(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    via: &str,
) -> Result<Result<ResolvedLever, String>, DbErr> {
    match resolve_lever(db, workspace_id, via).await {
        Ok(lever) => Ok(Ok(lever)),
        Err(LeverRefusal::Database(e)) => Err(e),
        Err(refusal) => Ok(Err(refusal.to_string())),
    }
}

/// The `via` the design names for `state` — resolved at dispatch, never here.
/// `None` for a design with no lever, which registration never lets run.
pub fn via_for(c: &ExperimentConfig, state: LeverState) -> Option<&str> {
    let l = c.lever.as_ref()?;
    Some(match state {
        LeverState::On => &l.on.via,
        LeverState::Off => &l.off.via,
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn rev() -> Uuid {
        Uuid::from_u128(9)
    }

    fn paths(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exactly_one_automation_resolves_to_its_path_and_the_revision() {
        let got = choose(
            "promo_on",
            rev(),
            &paths(&["automations/promo_on.automation.yml"]),
        )
        .unwrap();
        assert_eq!(
            got,
            ResolvedLever {
                file_path: "automations/promo_on.automation.yml".to_string(),
                revision_id: rev()
            }
        );
    }

    #[test]
    fn no_automation_of_that_name_is_refused_naming_it_and_the_revision() {
        let err = choose("promo_on", rev(), &[]).unwrap_err();
        assert!(
            matches!(&err, LeverRefusal::NotFound { via, revision } if via == "promo_on" && *revision == rev()),
            "{err:?}"
        );
        let message = err.to_string();
        assert!(
            message.contains("`promo_on`") && message.contains(&rev().to_string()),
            "{message}"
        );
        assert!(!err.is_fault());
    }

    #[test]
    fn an_ambiguous_lever_names_every_path() {
        // Review Focus 5. Two files share a `name:`; the runner could not know
        // which one the owner meant.
        let two = paths(&[
            "automations/a/promo_on.automation.yml",
            "automations/b/promo_on.automation.yml",
        ]);
        let err = choose("promo_on", rev(), &two).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("2 automations"), "{message}");
        assert!(
            message.contains("a/promo_on.automation.yml")
                && message.contains("b/promo_on.automation.yml"),
            "{message}"
        );
        assert!(message.contains("exactly one"), "{message}");
    }

    #[test]
    fn a_workspace_that_was_never_promoted_resolves_nothing() {
        let err = LeverRefusal::NoPromotedRevision {
            via: "promo_on".to_string(),
        };
        assert!(err.to_string().contains("no promoted revision"));
        assert!(!err.is_fault());
    }

    #[test]
    fn only_a_database_error_is_a_fault() {
        assert!(
            LeverRefusal::Database(sea_orm::DbErr::Custom("connection reset".to_string()))
                .is_fault()
        );
    }
}
