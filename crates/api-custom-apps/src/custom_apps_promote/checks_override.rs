//! `override_checks`: promote a build past its promote checks, with a reason.
//!
//! A request that moves production to a new build may carry `"override_checks":
//! {"reason": "…"}`. Accepted, it clears the gate's three blockers
//! (`checks_gate`: `checks_not_passed`, `checks_running`, `changes_not_seen`)
//! and nothing else: a build staging never served, a failed validation, an
//! unset required key, an unconfirmed schedule removal and the four-eyes rule
//! refuse the move as before. The checks themselves stay as they are.
//!
//! **Who.** Decided by `oxy-authz`, `Action::AppPromoteOverride`: an owner or
//! admin of the app's org, or Oxy staff holding `manage_apps` over it. Never
//! the org's service account or a CI run token acting as one (the ring), and
//! never a publish token: the legacy one is its minter with no narrowing
//! fact, so it is refused here by its marker, as is a token that holds an
//! `app_publish` grant. Asked BESIDE whatever the route needs to move
//! production at all.
//!
//! **Refusals.** `403 override_not_allowed` for a caller who may not; `400
//! override_reason_required` for a reason under [`MIN_REASON_CHARS`]
//! characters once trimmed.
//!
//! **Which requests carry it.** The promote route ([`ask`], whose plan also
//! shows whether the caller may), and the console's own production moves
//! ([`MoveChecks`]): Make Live, its rollback to any retained build, batch
//! publish and promote latest. The rollback route takes none: a rollback to a
//! build production served is never held. Neither does the promoting publish
//! or the partner console's promote: both are held like any move, and are
//! overridden on the app's own routes.
//!
//! **Only where the gate is on.** Where the org's `promote_checks_gate` flag
//! is off the field is ignored: nothing is held, so nothing is decided,
//! refused or audited.
//!
//! **Audited by the move.** An override that cleared something writes
//! `app.promote_checks_overridden` (`{build, reason, unmet, running,
//! changes_not_seen}`) in the transaction that moves production's pointer
//! (`production::move_resolved`), so a refused move leaves no row. An accepted
//! override that had nothing to clear writes none.

use entity::{app_builds, apps};
use oxy_app_core::audit::{AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_authz::{Action, Resource};
use oxy_server_authz::Caller;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use serde_json::json;

use super::checks_gate::Checks;
use super::error::PromoteError;
use crate::custom_apps_build_checks::kind::cap;
use crate::custom_apps_build_checks::settings;

/// The audit action of an override that cleared a check blocker.
pub const CHECKS_OVERRIDDEN: &str = "app.promote_checks_overridden";
/// The fewest characters a reason has, after trimming.
pub const MIN_REASON_CHARS: usize = 10;
/// The longest reason kept: it is a sentence in an audit row, not a document.
const MAX_REASON_CHARS: usize = 500;

/// A request's `override_checks`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OverrideRequest {
    #[serde(default)]
    pub reason: String,
}

/// An override that was asked by someone who may, with a reason: only
/// [`ask`] makes one, so a move cannot carry one nobody decided.
#[derive(Debug, Clone)]
pub struct AcceptedOverride {
    reason: String,
    actor: RequestActor,
}

impl AcceptedOverride {
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// How the promote checks gate applies to one production move.
#[derive(Debug, Clone, Default)]
pub enum ChecksAsk {
    /// Held by the gate, like any move.
    #[default]
    Gated,
    /// The request carried an accepted override.
    Overridden(AcceptedOverride),
    /// The dev seed's fixture (`seed_apps`): the platform's own example app,
    /// which no person promotes and no runner checks. Never a request's:
    /// only this crate can make one ([`ChecksAsk::fixture`]).
    Fixture(SeedFixture),
}

/// What makes a move the dev seed's. Not constructible outside this crate.
#[derive(Debug, Clone)]
pub struct SeedFixture(());

impl ChecksAsk {
    /// The dev seed's move of its own example app.
    pub(crate) fn fixture() -> Self {
        Self::Fixture(SeedFixture(()))
    }

    /// The accepted override, when the move carries one.
    pub fn accepted(&self) -> Option<&AcceptedOverride> {
        match self {
            Self::Overridden(accepted) => Some(accepted),
            Self::Gated | Self::Fixture(_) => None,
        }
    }
}

/// What a request asks of the gate, and whether its caller may override.
#[derive(Debug, Clone, Default)]
pub struct GateAsk {
    pub ask: ChecksAsk,
    /// `checks.overridable` in the plan.
    pub may_override: bool,
}

/// Whether the request's caller may override `app`'s checks: never with a
/// publish token, otherwise `Action::AppPromoteOverride`. Facts that cannot
/// be loaded refuse the request: there is no shipped check to fall back to.
async fn may_override(
    db: &DatabaseConnection,
    actor: &RequestActor,
    publish_token: bool,
    app: &apps::Model,
) -> Result<bool, PromoteError> {
    let caller = Caller::of(&actor.user, actor.credential.as_ref());
    if publish_token || caller.holds_app_publish() {
        return Ok(false);
    }
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(db, &caller, false)
        .await
        .ok_or_else(|| PromoteError::internal("load who is asking", "facts unavailable"))?;
    let resource = Resource::app(app.id, app.org_id).published_from(app.project_id);
    Ok(oxy_authz::require(&facts, Action::AppPromoteOverride, &resource).is_ok())
}

/// The reason as it is kept, or the refusal for one that is too short.
pub fn reason_of(request: &OverrideRequest) -> Result<String, PromoteError> {
    let reason = request.reason.trim();
    if reason.chars().count() < MIN_REASON_CHARS {
        return Err(PromoteError::OverrideReasonRequired);
    }
    Ok(cap(reason, MAX_REASON_CHARS))
}

/// What `request` asks of the gate for a promote of `app` to production.
/// `publish_token`: the request authenticated with a publish token.
///
/// Where the gate is off this decides nothing and answers the default. Where
/// it is on, the caller's standing is read (the plan shows it), and an
/// override is refused `403` from a caller who may not and `400` without a
/// reason.
pub async fn ask(
    db: &DatabaseConnection,
    actor: &RequestActor,
    publish_token: bool,
    app: &apps::Model,
    request: Option<&OverrideRequest>,
) -> Result<GateAsk, PromoteError> {
    if !settings::enabled_for(app.org_id) {
        return Ok(GateAsk::default());
    }
    let standing = may_override(db, actor, publish_token, app).await;
    let Some(request) = request else {
        // Only shown in the plan: a standing that could not be read says
        // "may not" there, and fails no plan.
        return Ok(GateAsk {
            ask: ChecksAsk::Gated,
            may_override: standing.unwrap_or(false),
        });
    };
    if !standing? {
        return Err(PromoteError::OverrideNotAllowed);
    }
    let accepted = AcceptedOverride {
        reason: reason_of(request)?,
        actor: actor.clone(),
    };
    Ok(GateAsk {
        ask: ChecksAsk::Overridden(accepted),
        may_override: true,
    })
}

/// What one of the console's own production moves asks of the gate: the
/// request's `override_checks`, and whether it came with a publish token.
/// The default asks nothing, and the move is held like any other.
#[derive(Debug, Clone, Copy, Default)]
pub struct MoveChecks<'a> {
    pub override_checks: Option<&'a OverrideRequest>,
    pub publish_token: bool,
}

impl MoveChecks<'_> {
    /// What the move of `app` carries. A request without an override decides
    /// nothing; one with it is [`ask`]ed, and refused as there.
    pub async fn ask(
        &self,
        db: &DatabaseConnection,
        actor: &RequestActor,
        app: &apps::Model,
    ) -> Result<ChecksAsk, PromoteError> {
        match self.override_checks {
            None => Ok(ChecksAsk::Gated),
            Some(request) => Ok(ask(db, actor, self.publish_token, app, Some(request))
                .await?
                .ask),
        }
    }
}

/// The audit row of an override that cleared `checks` for a move of `app`'s
/// production to `build`.
pub(super) fn entry(
    accepted: &AcceptedOverride,
    app: &apps::Model,
    build: &app_builds::Model,
    checks: &Checks,
) -> AuditEntry {
    let production = AppEnvironment::Production;
    let blocked: Vec<&str> = checks
        .blockers(&build.build_id)
        .iter()
        .map(|blocker| blocker.code)
        .collect();
    AuditEntry::for_request(&accepted.actor, CHECKS_OVERRIDDEN)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{production}", app.id),
            format!("{}/{production}", app.slug),
        )
        .environment(production.name())
        .metadata(json!({
            "build": build.id,
            "reason": accepted.reason,
            "unmet": checks.unmet,
            "running": checks.running,
            "changes_not_seen": blocked.contains(&super::error::CHANGES_NOT_SEEN),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asking(reason: &str) -> OverrideRequest {
        OverrideRequest {
            reason: reason.to_string(),
        }
    }

    /// Ten characters after trimming, and the kept reason is trimmed and capped.
    #[test]
    fn a_reason_is_at_least_ten_characters_after_trimming() {
        for short in ["", "   ", "hotfix", " 123456789 ", "\n\t  too short\t"] {
            let refused = reason_of(&asking(short)).expect_err(short);
            assert_eq!(refused.code(), "override_reason_required", "{short:?}");
            assert_eq!(refused.status().as_u16(), 400);
        }
        assert_eq!(reason_of(&asking(" 1234567890 ")).unwrap(), "1234567890");
        let kept = reason_of(&asking("  outage: shipping the fix now  ")).unwrap();
        assert_eq!(kept, "outage: shipping the fix now");
        let long = reason_of(&asking(&"x".repeat(5_000))).unwrap();
        assert_eq!(long.chars().count(), MAX_REASON_CHARS);
        // A body that names no reason is the short one, never a parse error.
        let absent: OverrideRequest = serde_json::from_value(json!({})).unwrap();
        assert!(reason_of(&absent).is_err());
    }
}
