//! Who is mailed about a token (API-tokens design §8 Phase 5): the owner of a
//! personal token or legacy key; the active admins and owners of the org, for
//! a token that acts as a service account — the account has no inbox, and its
//! org's officers are the ones who can act on it.

use entity::org_members::OrgRole;
use entity::prelude::{OrgMembers, Organizations, ServiceAccounts, Users};
use entity::users::UserStatus;
use entity::{api_tokens, org_members, users};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::dto;
use super::error::TokenError;
use crate::emails::token_mail::Audience;

/// The addresses to mail about one token, and how to address them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Recipients {
    pub emails: Vec<String>,
    /// `(org name, account name)` for a service-account token.
    pub account: Option<(String, String)>,
}

impl Recipients {
    pub fn audience(&self) -> Audience<'_> {
        match &self.account {
            Some((org_name, account)) => Audience::OrgAdmins { org_name, account },
            None => Audience::Owner,
        }
    }
}

/// The active users among `ids` that have an address.
async fn addresses<C: ConnectionTrait>(db: &C, ids: Vec<Uuid>) -> Result<Vec<String>, TokenError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let found = Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?;
    let mut emails: Vec<String> = found
        .into_iter()
        .filter(|u| u.status == UserStatus::Active)
        .filter_map(|u| u.email)
        .filter(|e| !e.trim().is_empty())
        .collect();
    emails.sort();
    emails.dedup();
    Ok(emails)
}

/// Who to mail about `row`. Empty when nobody can be: an owner with no
/// address, or an org with no active officer.
pub(crate) async fn for_token<C: ConnectionTrait>(
    db: &C,
    row: &api_tokens::Model,
) -> Result<Recipients, TokenError> {
    if !dto::acts_as_account(row) {
        return Ok(Recipients {
            emails: addresses(db, vec![row.principal_user_id]).await?,
            account: None,
        });
    }
    let Some(account) = ServiceAccounts::find_by_id(row.principal_user_id)
        .one(db)
        .await?
    else {
        return Ok(Recipients::default());
    };
    let org_name = Organizations::find_by_id(account.org_id)
        .one(db)
        .await?
        .map(|o| o.name)
        .unwrap_or_default();
    let officers: Vec<Uuid> = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(account.org_id))
        .filter(org_members::Column::Role.is_in([OrgRole::Owner, OrgRole::Admin]))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect();
    Ok(Recipients {
        emails: addresses(db, officers).await?,
        account: Some((org_name, account.name)),
    })
}
