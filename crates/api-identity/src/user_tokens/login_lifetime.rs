//! How long an `oxyc login` lasts for one person.
//!
//! A year ([`LOGIN_LIFETIME_DAYS`]), unless an org the person belongs to caps
//! token lifetimes tighter; then that cap.
//!
//! The cap has to be read here because of what happens otherwise. A login is
//! an all-access personal token, and one that outlives an org's
//! `max_lifetime_days` is not refused: it goes inert in that org
//! (`oxy_auth::token::policy::violation`), and nothing says so at login. A
//! year-long login would stop working in its owner's own org wherever the cap
//! is under a year. A shorter login that works there is the better answer.

use chrono::Duration;
use oxy_auth::token::personal::LOGIN_LIFETIME_DAYS;
use oxy_auth::token::policy_store;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::error::TokenError;

/// [`LOGIN_LIFETIME_DAYS`], or `cap` days when that is shorter.
fn days_under(cap: Option<i32>) -> i64 {
    cap.map_or(LOGIN_LIFETIME_DAYS, |cap| {
        LOGIN_LIFETIME_DAYS.min(i64::from(cap))
    })
}

/// The lifetime of a login minted now for `user_id`: the tightest lifetime cap
/// among the orgs they are a member of, and never more than a year.
pub(super) async fn of<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Duration, TokenError> {
    let orgs = policy_store::member_orgs(db, &[user_id])
        .await?
        .remove(&user_id)
        .unwrap_or_default();
    let cap = policy_store::tightest_cap_in(db, &orgs).await?;
    Ok(Duration::days(days_under(cap)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_lasts_a_year_where_no_org_caps_it() {
        assert_eq!(days_under(None), 365);
    }

    #[test]
    fn a_tighter_cap_shortens_it_and_a_looser_one_does_not_lengthen_it() {
        assert_eq!(days_under(Some(180)), 180);
        assert_eq!(days_under(Some(30)), 30);
        assert_eq!(days_under(Some(365)), 365);
        assert_eq!(days_under(Some(3650)), 365);
    }
}
