//! Does this person run anything? — the one question the notification
//! surfaces segment by, asked of `oxy-authz` and nowhere else.
//!
//! The line is `Action::FactoryUse` / `Ring::Factory`: whoever may open Nexus.
//! Drawn there rather than on an org role because that is the authority that
//! already decides who builds and runs a workspace, and because asking it
//! through `oxy_server_authz::factory` is what stops this drifting from the
//! Factory gate itself. **No role is read by hand here** — there is no
//! `matches!(role, Owner | Admin)` in this crate, and
//! `crates/app/tests/authz/authz_boundaries.rs` would fail the build if there
//! were.
//!
//! The answer is a [`Standing`]: three booleans, each the question an
//! [`Audience`] asks, resolved **globally** over the person's own orgs. Global
//! because the inbox is global — it spans every org a person belongs to — so an
//! admin in one org and a plain member in another is an operator, and filtering
//! to the second org simply shows them an empty Operations list.
//!
//! What is **not** here: nothing narrows who a notification is sent to. Every
//! source keeps the recipients it has today; a workspace admin who is a plain
//! org member is still told when a run fails, which is the finding the design's
//! builder review turned up (§3.1). The audience decides what the catalogue and
//! the settings document draw, and which stream a notice is in. When a kind
//! does need its recipients narrowed, the helper to write is a `narrow(kind,
//! org, recipients) -> recipients` beside this — one shared filter that can only
//! ever remove someone, never add them.

use oxy_server_authz::caller::Caller;
use oxy_server_authz::factory::{may_use_factory, may_use_factory_in_org};
use oxy_server_authz::loader::load_principal_facts;
use oxy_server_authz::{Action, PrincipalFacts, Resource, allows};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::catalog::source::Audience;

/// What a person stands as somewhere — each field the question one [`Audience`]
/// asks. Copy, because every source of their catalogue asks the same three.
///
/// The three nest (`owner` → `officer` → `operator`), which
/// [`Standing::nests`] asserts rather than assumes: they are computed
/// independently so that a change to the authority model shows up as a failing
/// test instead of a silently wrong tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing {
    /// May open Nexus in at least one of their orgs, or in a workspace one was
    /// granted to them.
    pub operator: bool,
    /// Owner or admin of at least one of their orgs.
    pub officer: bool,
    /// Owner of at least one of their orgs.
    pub owner: bool,
}

impl Standing {
    /// What unknown standing means on a **read**: show everything.
    ///
    /// `load_principal_facts` answers `None` for "we could not ask" — no
    /// connection, or a lookup that errored — which is deliberately not the
    /// fact "holds nothing". Noise is recoverable; a hidden switch for a notice
    /// you are receiving is not, so a blip opens the tree rather than pruning
    /// it.
    pub const EVERYTHING: Self = Self {
        operator: true,
        officer: true,
        owner: true,
    };

    /// Whether this standing admits someone to `audience`.
    ///
    /// Never for [`Audience::Addressed`]: that kind is only ever sent to named
    /// individuals, so no standing admits it and it reaches a person's
    /// catalogue by the other clause — having received one — instead.
    pub fn admits(self, audience: Audience) -> bool {
        match audience {
            Audience::Everyone => true,
            Audience::Operators => self.operator,
            Audience::Officers => self.officer,
            Audience::Owners => self.owner,
            Audience::Addressed => false,
        }
    }

    /// Whether the three nest as the lattice says. Asserted by the tests here
    /// and by nothing else: no behaviour depends on it.
    #[cfg(test)]
    fn nests(self) -> bool {
        (!self.owner || self.officer) && (!self.officer || self.operator)
    }
}

/// `caller`'s [`Standing`]. Unknown facts answer [`Standing::EVERYTHING`].
///
/// The caller is the request's own — `AuthenticatedUser` carries the credential
/// it arrived with, so a token's ceiling narrows this exactly as it narrows
/// every other decision. A narrowed token therefore sees a view that is
/// *narrower* than the send decision, never wider, which is the safe direction.
pub async fn standing_of(db: &DatabaseConnection, caller: &Caller) -> Standing {
    let Some(facts) = load_principal_facts(db, caller).await else {
        return Standing::EVERYTHING;
    };
    let granted = granted_workspaces(db, &facts).await.unwrap_or_else(|e| {
        // One query, and only a grantee's answer depends on it. Losing it can
        // only make `operator` read false for someone whose standing comes from
        // a grant alone, so say so rather than pretending the grant is absent.
        tracing::warn!(error = %e, "notification operator standing: factory grants unreadable");
        Vec::new()
    });
    standing_from(&facts, &granted)
}

/// Each workspace `facts` holds a Factory grant on, with its org — the pairs
/// [`may_use_factory`] needs. Empty for everyone who holds no grant, which is
/// almost everyone, and then no query is made at all.
async fn granted_workspaces(
    db: &DatabaseConnection,
    facts: &PrincipalFacts,
) -> Result<Vec<(Uuid, Uuid)>, DbErr> {
    if facts.factory_workspaces.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(Uuid, Option<Uuid>)> = entity::workspaces::Entity::find()
        .select_only()
        .column(entity::workspaces::Column::Id)
        .column(entity::workspaces::Column::OrgId)
        .filter(entity::workspaces::Column::Id.is_in(facts.factory_workspaces.iter().copied()))
        .into_tuple()
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, org)| org.map(|org| (id, org)))
        .collect())
}

/// The three questions, over facts already loaded. Pure, so the whole lattice
/// is testable without a database.
fn standing_from(facts: &PrincipalFacts, granted: &[(Uuid, Uuid)]) -> Standing {
    let in_own_orgs = |ask: &dyn Fn(Uuid) -> bool| facts.member_orgs.iter().copied().any(ask);
    // Only the person's OWN orgs are probed. Staff standing reaches into a
    // tenant, but a notification is filed under an org and every source sends
    // to members, named individuals or grantees — so an Oxy operator with no
    // membership has no tenant notices in their personal inbox to segment, and
    // there is nothing for a platform-scoped term to answer about here.
    Standing {
        operator: in_own_orgs(&|org| may_use_factory_in_org(facts, org))
            || granted
                .iter()
                .any(|(workspace, org)| may_use_factory(facts, *workspace, *org)),
        // `Ring::OrgAdminStrict` is the officer term itself: a person whose
        // `org_members` role is owner or admin, as far as their token reaches.
        officer: in_own_orgs(&|org| allows(facts, Action::OrgBilling, &Resource::org(org))),
        // `Ring::OwnerOnly`. Its staff term requires NOT being a member, and
        // only member orgs are probed, so this is a real owner and nobody else.
        owner: in_own_orgs(&|org| allows(facts, Action::OrgOwnerManage, &Resource::org(org))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_server_authz::{PlatformRole, PlatformStanding, Scope};

    const ORG: Uuid = Uuid::from_u128(1);
    const OTHER: Uuid = Uuid::from_u128(2);
    const WS: Uuid = Uuid::from_u128(3);

    fn standing(facts: &PrincipalFacts, granted: &[(Uuid, Uuid)]) -> Standing {
        let got = standing_from(facts, granted);
        assert!(
            got.nests(),
            "{got:?} must nest owner -> officer -> operator"
        );
        got
    }

    fn owner() -> PrincipalFacts {
        PrincipalFacts {
            owned_orgs: vec![ORG],
            admin_orgs: vec![ORG],
            member_orgs: vec![ORG],
            ..Default::default()
        }
    }

    fn admin() -> PrincipalFacts {
        PrincipalFacts {
            admin_orgs: vec![ORG],
            member_orgs: vec![ORG],
            ..Default::default()
        }
    }

    fn member() -> PrincipalFacts {
        PrincipalFacts {
            member_orgs: vec![ORG],
            ..Default::default()
        }
    }

    #[test]
    fn an_owner_an_admin_and_a_grantee_are_operators_and_a_plain_member_is_not() {
        assert_eq!(
            standing(&owner(), &[]),
            Standing {
                operator: true,
                officer: true,
                owner: true
            }
        );
        assert_eq!(
            standing(&admin(), &[]),
            Standing {
                operator: true,
                officer: true,
                owner: false
            }
        );
        assert_eq!(standing(&member(), &[]), Standing::default());
        let granted = PrincipalFacts {
            factory_workspaces: vec![WS],
            ..member()
        };
        assert_eq!(
            standing(&granted, &[(WS, ORG)]),
            Standing {
                operator: true,
                officer: false,
                owner: false
            },
            "the Factory in one workspace, and no standing in the org at all"
        );
    }

    #[test]
    fn a_grant_in_an_org_they_do_not_belong_to_makes_nobody_an_operator() {
        // `Ring::Factory` ANDs the grant with membership, so a row left behind
        // by a removed member opens nothing — including this.
        let stale = PrincipalFacts {
            factory_workspaces: vec![WS],
            ..Default::default()
        };
        assert!(!standing(&stale, &[(WS, ORG)]).operator);
        // And a grant for some other org's workspace is not this org's.
        let elsewhere = PrincipalFacts {
            factory_workspaces: vec![WS],
            ..member()
        };
        assert!(!standing(&elsewhere, &[(WS, OTHER)]).operator);
    }

    #[test]
    fn standing_anywhere_is_standing_the_inbox_is_global() {
        let admin_elsewhere = PrincipalFacts {
            admin_orgs: vec![OTHER],
            member_orgs: vec![ORG, OTHER],
            ..Default::default()
        };
        let got = standing(&admin_elsewhere, &[]);
        assert!(got.operator && got.officer && !got.owner);
    }

    #[test]
    fn staff_with_no_membership_stand_as_nothing_here() {
        // Deliberate: every platform notice is filed under an org, and no
        // source sends one to a non-member, so there is nothing to segment.
        let staff = PrincipalFacts {
            platform: Some(PlatformStanding::from_role(
                PlatformRole::GlobalAdmin,
                Scope::All,
            )),
            ..Default::default()
        };
        assert_eq!(standing(&staff, &[]), Standing::default());
    }

    #[test]
    fn each_audience_asks_one_question_and_addressed_asks_none() {
        let nobody = Standing::default();
        let everyone = Standing::EVERYTHING;
        for audience in [
            Audience::Everyone,
            Audience::Operators,
            Audience::Officers,
            Audience::Owners,
            Audience::Addressed,
        ] {
            assert!(
                nobody.admits(audience) == (audience == Audience::Everyone),
                "{audience:?} admits a plain member only if it is Everyone"
            );
            assert!(
                everyone.admits(audience) == (audience != Audience::Addressed),
                "{audience:?} is admitted by standing unless it is Addressed"
            );
        }
        let operator = Standing {
            operator: true,
            ..Default::default()
        };
        assert!(operator.admits(Audience::Operators));
        assert!(!operator.admits(Audience::Officers) && !operator.admits(Audience::Owners));
    }
}
