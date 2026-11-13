//! What a lifecycle row says about a token's **access**: its flags and its
//! grants (API-tokens design §3.7). Used by `token.created` and by the before
//! and after of `token.grants_changed`.
//!
//! **A row lists only its own org's grants.** A lifecycle event is written
//! once per org the token reaches, and a token may hold grants in several. A
//! workspace or an app of one org is not for another org's chain to hold, so
//! the list is built per row: org X's row lists the grants that name org X
//! and nothing of any other org's, not even how many there are. The flags are
//! the token's own and are the same on every row.
//!
//! No view serves these lists: a key's Activity shows an event's action and
//! three metadata keys, to its owner and to an org's admins alike
//! (`api_keys::activity`). A reader that is one day given the whole of a
//! token's access, as its owner may be, must read every row of the event: the
//! rows share `metadata.event_id`, and each org the token reaches has one (up
//! to `audit::FANOUT_MAX_ORGS`).

use entity::{api_token_grants, api_tokens};
use serde_json::{Value, json};
use uuid::Uuid;

use super::audit::Own;

/// A token's access at one moment. It copies the flags and borrows only the
/// grants, so the "before" of an edit outlives the row the edit consumes.
pub(crate) struct Access<'a> {
    all_access: bool,
    platform: bool,
    partner: bool,
    grants: &'a [api_token_grants::Model],
}

impl<'a> Access<'a> {
    pub(crate) fn of(token: &api_tokens::Model, grants: &'a [api_token_grants::Model]) -> Self {
        Self {
            all_access: token.all_access,
            platform: token.platform,
            partner: token.partner,
            grants,
        }
    }

    /// The live grants that name `org`, by id. None for a row with no org, and
    /// none for an all-access token: its grants are not what it reaches.
    fn grants_in(&self, org: Option<Uuid>) -> Vec<Value> {
        self.grants
            .iter()
            .filter(|g| g.revoked_at.is_none() && !self.all_access)
            .filter(|g| Some(g.org_id) == org)
            .map(|g| {
                json!({
                    "kind": g.kind,
                    "org_id": g.org_id,
                    "workspace_id": g.workspace_id,
                    "role_ceiling": g.role_ceiling,
                    "app_id": g.app_id,
                })
            })
            .collect()
    }

    /// What the row of `org` says: the flags, and that org's grants alone.
    pub(crate) fn in_org(&self, org: Option<Uuid>) -> Value {
        json!({
            "all_access": self.all_access,
            "platform": self.platform,
            "partner": self.partner,
            "grants": self.grants_in(org),
        })
    }
}

/// `token.created`: each row's metadata carries the access in its own org.
pub(crate) fn created<'a>(access: &'a Access<'a>) -> impl Fn(Option<Uuid>) -> Own + 'a {
    move |org| Own::detail(access.in_org(org))
}

/// `token.grants_changed`: each row's before and after are the access in its
/// own org. An org the edit did not touch reads the same grants on both sides.
pub(crate) fn changed<'a>(
    before: &'a Access<'a>,
    after: &'a Access<'a>,
) -> impl Fn(Option<Uuid>) -> Own + 'a {
    move |org| Own::change(before.in_org(org), after.in_org(org))
}

#[cfg(test)]
#[path = "access_audit_tests.rs"]
mod tests;
