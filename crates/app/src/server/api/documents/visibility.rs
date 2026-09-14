//! The one filter every document read goes through.
//!
//! There is exactly one function that decides which rows a caller may see, and
//! every read — list, get, download, and the search endpoint that arrives with
//! PR 3 — composes it. A second place to answer this question is how a search
//! endpoint ends up returning what the list endpoint hides, which is the
//! failure this module exists to make structurally impossible.
//!
//! # Why a filter and not a ring
//!
//! Two reasons, and the second is the load-bearing one:
//!
//! * The readable set is unbounded per user. Putting it in `PrincipalFacts`
//!   would mean an unbounded read on every request that touches authz.
//! * The reader usually holds no `org_members` row. A frontline worker is
//!   enrolled by PIN on a shared device precisely so that they do not become an
//!   org member — membership would hand them Airhouse settings and, through
//!   `EffectiveWorkspaceRole`, Databases and Secrets. A gate written as "is a
//!   member of this org" locks out the entire audience of a Knowledge base.
//!
//! `oxy-authz` still owns WRITES (`Action::ManageDocuments`). It owns nothing
//! here.
//!
//! # How this is tested
//!
//! [`resolve_standing`] reads the database, and the condition it feeds is SQL,
//! so the real proof is `crates/app/tests/platform/documents.rs` against seeded
//! rows — a worker at one store must not see another store's documents. The
//! unit test below covers only the property that can be checked without a
//! database, which is also the most dangerous one: no standing must produce no
//! query, not an unfiltered one.

use entity::{documents, folders, org_frontline_members, org_members, org_role_members, users};

use crate::server::api::admin::assume;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

/// What one caller may see in one org.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadStanding {
    /// A real `org_members` row. Sees everything published, `hq` included,
    /// plus drafts they authored — or every draft, if they are an officer.
    Member { is_officer: bool },
    /// Standing without membership: enrolled by PIN, or holding a
    /// tenant-defined role, or both. Sees published org-visible documents at
    /// the locations they are rostered at.
    ///
    /// `org_wide` is set by a franchisor-scope role, which is held across the
    /// org rather than at one store — a Corporate user, in the vocabulary the
    /// assignment graph uses.
    Frontline {
        locations: Vec<Uuid>,
        org_wide: bool,
    },
    /// No standing at all. Every read answers empty or 404.
    None,
}

/// Resolve what the caller is, in this org, right now.
///
/// Errors propagate rather than degrading to [`ReadStanding::None`].
/// Fail-closed is right for the decision and wrong for a transient fault: a
/// database blip that reads as "no standing" answers `404 no such document`
/// for a document that exists, which is the least legible failure available and
/// looks to the reader like their SOP was deleted.
pub async fn resolve_standing(
    db: &DatabaseConnection,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<ReadStanding, DbErr> {
    if let Some(m) = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(db)
        .await?
    {
        return Ok(ReadStanding::Member {
            is_officer: matches!(
                m.role,
                org_members::OrgRole::Owner | org_members::OrgRole::Admin
            ),
        });
    }

    // An operator inside an assume-role session — AFTER the membership lookup,
    // and only when there is none.
    //
    // `OrgAdmin` reaches this org through a synthesised Owner membership that
    // `org_context` mints for Oxy staff and partners holding a live, audited
    // session. This function required a real `org_members` row, which such a
    // caller by definition does not have, so every write was accepted and every
    // read answered `404`.
    //
    // The PLACEMENT is load-bearing, and the first version of this fix got it
    // wrong by consulting assume first, unconditionally. `org_context` consults
    // it only in the `None` arm of the membership lookup — a real member always
    // wins there — and `assume::start` does not refuse a real member, so
    // somebody can hold both: an Oxy employee who is a Member of a pilot org, a
    // partner who is a Member of a client org. For them the two halves diverged
    // in the worst direction: `OrgAdmin` denied the write on their real Member
    // role while this granted `is_officer`, handing them every draft and the
    // trash. Starting a session elevated reads it was never meant to touch, and
    // `is_global_override` stayed unset — so the flags that close billing and
    // admin promotion to an assuming operator stayed off too.
    //
    // Same two calls `org_context` makes, in the same order, now in the same
    // position. `may_act_as` is re-checked per request on both sides, so
    // revoking a partner's capability closes the read at once rather than at
    // expiry.
    //
    // The email is looked up here rather than threaded through nine call sites
    // and every document test; `may_act_as` needs it because a Global Owner is
    // identified by address. `is_session_live` runs on every document read —
    // one indexed lookup (`idx_admin_assume_actor_org`) — and gates the rest.
    if assume::is_session_live(db, user_id, org_id).await
        && let Some(email) = users::Entity::find_by_id(user_id)
            .one(db)
            .await?
            .and_then(|u| u.email)
        && let Some(authority) = assume::may_act_as(db, user_id, &email, org_id).await
    {
        return Ok(ReadStanding::Member {
            is_officer: matches!(
                authority.org_role(),
                org_members::OrgRole::Owner | org_members::OrgRole::Admin
            ),
        });
    }

    let roles = org_role_members::Entity::find()
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .filter(org_role_members::Column::UserId.eq(user_id))
        .all(db)
        .await?;

    let frontline = org_frontline_members::Entity::find()
        .filter(org_frontline_members::Column::OrgId.eq(org_id))
        .filter(org_frontline_members::Column::UserId.eq(user_id))
        .one(db)
        .await?;

    // Suspension is a REVOCATION, so it outranks anything else this person
    // holds. Suspending a worker removes their access without deleting the row
    // and the history attached to it, and the previous shape — filtering the
    // query on `status = 'active'` and then asking `roles.is_empty() &&
    // !enrolled` — did not express that: a suspended worker who also held a
    // tenant-defined role read as merely "not enrolled", the role carried them
    // past the guard, and they kept the handbook. The status has to be a branch
    // rather than a filter, because a filter can only make the row absent and
    // absent is exactly what a never-enrolled roleholder looks like.
    if frontline
        .as_ref()
        .is_some_and(|f| f.status.as_str() != org_frontline_members::STATUS_ACTIVE)
    {
        return Ok(ReadStanding::None);
    }

    // A role alone is standing — that is the documented model, "enrolled by
    // PIN, or holding a tenant-defined role, or both" — but only for somebody
    // whose enrolment was never revoked, which the branch above has settled.
    if roles.is_empty() && frontline.is_none() {
        return Ok(ReadStanding::None);
    }

    Ok(ReadStanding::Frontline {
        org_wide: roles.iter().any(|r| r.location_id.is_none()),
        locations: roles.iter().filter_map(|r| r.location_id).collect(),
    })
}

/// # Open, since main's operating graph landed: places are a TREE now
///
/// `location_id` is matched literally against the locations on the caller's
/// roster rows. That was total when `locations` was flat. `#3113` gave them
/// `parent_id` and a free-form `kind`, so a tenant can now file a document at
/// "Northeast" — and every worker rostered at a Northeast *store* sees nothing,
/// because their roster names the store and the document names its parent. The
/// write answers `201` and the document is unreadable by the people it is for.
///
/// Closed at the WRITE, not here. `gate_refs` refuses a place that has children:
/// having children is the structural half of the question and the half that is
/// decidable, because no roster points at a container. A childless place with
/// nobody rostered at it is still accepted — that is every store on its first
/// day.
///
/// The read side is deliberately unchanged. Whether a regional SOP should reach
/// the stores under it is a product question, and answering it in this filter
/// would put it at odds with `operating_graph::reach`, which is flat by design
/// and documented as such. Refusing the write fails loudly at the moment
/// somebody makes the mistake, instead of quietly at every read afterwards —
/// and leaves that question open to be answered on purpose.
///
/// The condition every read composes, or `None` when the caller sees nothing.
///
/// Returning `Option` rather than a condition that happens to match no rows is
/// deliberate: a caller with no standing must be a branch the handler takes,
/// not a filter it trusts to be empty. An `Option` that is ignored does not
/// compile; a condition that is ignored returns the whole table.
pub fn visible_documents(org_id: Uuid, caller: Uuid, standing: &ReadStanding) -> Option<Condition> {
    visible_documents_scoped(org_id, caller, standing, Trash::Excluded)
}

/// Which side of the trash a read is asking about.
///
/// A named type rather than a `bool`, because `visible_documents(org, me,
/// &standing, true)` at a call site says nothing about what `true` means, and
/// this is the one parameter that decides whether deleted content comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trash {
    /// Live rows. Every ordinary read.
    Excluded,
    /// Trashed rows only — the "Deleted" tab, and the only way to reach the
    /// restore endpoint.
    Only,
}

/// The filter with the trash side chosen.
///
/// Reading the trash is **officers only**, and that is a policy decision worth
/// stating rather than a filter detail. A deleted document is one somebody
/// decided the org should stop seeing; handing it back to a frontline worker
/// through a different tab would undo the decision without anybody noticing.
/// Every other standing gets the live filter no matter what it asks for, which
/// fails closed rather than refusing — a worker who somehow requests the trash
/// sees their ordinary library, not an error that tells them a trash exists.
pub fn visible_documents_scoped(
    org_id: Uuid,
    caller: Uuid,
    standing: &ReadStanding,
    trash: Trash,
) -> Option<Condition> {
    let officer = matches!(standing, ReadStanding::Member { is_officer: true });
    let want_trash = trash == Trash::Only && officer;

    let base = Condition::all()
        .add(documents::Column::OrgId.eq(org_id))
        .add(if want_trash {
            documents::Column::DeletedAt.is_not_null()
        } else {
            documents::Column::DeletedAt.is_null()
        });

    match standing {
        ReadStanding::None => None,

        // An officer sees the library as it is, drafts included — they are the
        // people the Drafts tab exists for, and the only ones the trash opens
        // for.
        ReadStanding::Member { is_officer: true } => Some(base),

        // A plain member sees everything published, and their own unfinished
        // work. Somebody else's draft is not theirs to read.
        ReadStanding::Member { is_officer: false } => Some(
            base.add(
                Condition::any()
                    .add(documents::Column::Status.eq("published"))
                    .add(documents::Column::CreatedBy.eq(caller)),
            ),
        ),

        ReadStanding::Frontline {
            locations,
            org_wide,
        } => {
            let mut cond = base
                .add(documents::Column::Status.eq("published"))
                // `hq` is the whole reason this column exists: head-office
                // material that a store's staff must not see.
                .add(documents::Column::Visibility.eq("org"));

            if !org_wide {
                // Org-wide documents, plus this person's own stores. A worker
                // rostered nowhere still sees the org-wide handbook, which is
                // the correct answer for somebody enrolled but not yet placed.
                let mut scope = Condition::any().add(documents::Column::LocationId.is_null());
                if !locations.is_empty() {
                    scope = scope.add(documents::Column::LocationId.is_in(locations.clone()));
                }
                cond = cond.add(scope);
            }
            Some(cond)
        }
    }
}

/// The same decision for the folder tree.
///
/// Kept beside [`visible_documents`] rather than inlined in the handler for the
/// same reason that one exists: the tree and the listing must agree about `hq`,
/// and two answers in two files is how they stop agreeing. A folder is a
/// container, so this is deliberately coarser — it has no draft state and no
/// location scope, and a visible folder may still be empty for the caller.
pub fn visible_folders(org_id: Uuid, standing: &ReadStanding) -> Option<Condition> {
    visible_folders_scoped(org_id, standing, Trash::Excluded)
}

/// The folder tree with the trash side chosen.
///
/// Same policy as [`visible_documents_scoped`], and stated once there: reading
/// the trash is officers only, and a non-officer asking for it gets their
/// ordinary tree rather than a refusal. A trashed folder has to be listable by
/// somebody or `restore_folder` is a route nothing can ever call — which it was:
/// the app offered "trash this folder" and had no way back.
pub fn visible_folders_scoped(
    org_id: Uuid,
    standing: &ReadStanding,
    trash: Trash,
) -> Option<Condition> {
    let officer = matches!(standing, ReadStanding::Member { is_officer: true });
    let side = if officer && trash == Trash::Only {
        folders::Column::DeletedAt.is_not_null()
    } else {
        folders::Column::DeletedAt.is_null()
    };
    let base = Condition::all()
        .add(folders::Column::OrgId.eq(org_id))
        .add(side);

    match standing {
        ReadStanding::None => None,
        ReadStanding::Member { .. } => Some(base),
        ReadStanding::Frontline { .. } => Some(base.add(folders::Column::Visibility.eq("org"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one property worth asserting without a database, and the one whose
    /// failure is worst: a caller with no standing must produce NO query. If
    /// this ever returns `Some`, every handler that composes it starts reading
    /// an org the caller has nothing to do with.
    #[test]
    fn no_standing_produces_no_query_at_all() {
        assert!(
            visible_documents(Uuid::from_u128(1), Uuid::from_u128(2), &ReadStanding::None)
                .is_none()
        );
    }

    /// The other three all produce one, so the `None` above is a real branch
    /// rather than the only thing this function ever does.
    #[test]
    fn every_real_standing_produces_a_query() {
        for s in [
            ReadStanding::Member { is_officer: true },
            ReadStanding::Member { is_officer: false },
            ReadStanding::Frontline {
                locations: vec![Uuid::from_u128(9)],
                org_wide: false,
            },
        ] {
            assert!(visible_documents(Uuid::from_u128(1), Uuid::from_u128(2), &s).is_some());
            assert!(visible_folders(Uuid::from_u128(1), &s).is_some());
        }
    }

    /// Asking for the trash is not the same as being allowed it. A worker who
    /// requests deleted rows gets their ordinary library — failing closed
    /// without announcing that a trash exists.
    #[test]
    fn only_an_officer_can_ask_for_the_trash() {
        let org = Uuid::from_u128(1);
        let me = Uuid::from_u128(2);

        // The officer's trash query and their live query must differ, or the
        // "Deleted" tab is showing the same rows as "Home".
        let officer = ReadStanding::Member { is_officer: true };
        assert_ne!(
            format!(
                "{:?}",
                visible_documents_scoped(org, me, &officer, Trash::Only)
            ),
            format!(
                "{:?}",
                visible_documents_scoped(org, me, &officer, Trash::Excluded)
            )
        );

        // For everyone else the two are identical: the request is ignored, not
        // refused.
        for s in [
            ReadStanding::Member { is_officer: false },
            ReadStanding::Frontline {
                locations: vec![],
                org_wide: false,
            },
        ] {
            assert_eq!(
                format!("{:?}", visible_documents_scoped(org, me, &s, Trash::Only)),
                format!(
                    "{:?}",
                    visible_documents_scoped(org, me, &s, Trash::Excluded)
                ),
                "a non-officer was given a different query for the trash"
            );
        }
    }

    /// The tree obeys the same "no standing, no query" rule as the listing.
    /// Stated separately because it is a separate function, and a folder tree
    /// that leaked would name every SOP the caller cannot open.
    #[test]
    fn no_standing_sees_no_folders_either() {
        assert!(visible_folders(Uuid::from_u128(1), &ReadStanding::None).is_none());
    }
}
