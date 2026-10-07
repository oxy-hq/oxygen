//! Who a report is emailed to, and the sending.
//!
//! **Who.** Every staff address that holds [`AUDIENCE`] — the capability the
//! report's console page is gated on, so nobody is mailed a link they cannot
//! open — except those the email is turned off for, by themselves or by an
//! admin. That is the Global Owners and the Global Admins; an App Operator
//! ships apps and is not an audience for a cross-tenant report.
//!
//! **What each one gets.** The report narrowed to the orgs their grant
//! reaches: capabilities gate verbs, scope filters rows, and a mail is rows.
//!
//! **At most once.** A send is claimed by inserting `(report, address)` before
//! the provider is called, so two nodes passing at once send one mail between
//! them. A failed send gives its claim back and the next pass tries again; a
//! process that dies mid-send keeps its claim, and that address misses the
//! week rather than risk getting it twice.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use oxy_authz::{Cap, Scope};
use sea_orm::{DatabaseConnection, DbErr};

use crate::emails::usage_report::{DeliveryMode, ReportMailer};
use crate::server::authz::globals::{self, StaffAddress};

use super::email;
use super::store::{self, Preference, StoredReport};

/// The capability that makes someone a reader of this report. The router gates
/// the console page on `Action::PlatformOperate`, which is this capability.
pub const AUDIENCE: Cap = Cap::OperatePlatform;

/// How long one pass may spend mailing. The rest is sent on the next pass.
const MAIL_BUDGET: Duration = Duration::from_secs(20);
/// How long one send may take before it counts as failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// What a delivery pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delivered {
    pub sent: usize,
    pub failed: usize,
    /// Readers whose slice of the report had nothing in it to send.
    pub nothing_to_say: usize,
}

/// One person the report is for, and whether it is emailed to them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipient {
    pub staff: StaffAddress,
    pub enabled: bool,
    /// Who last changed `enabled`, and when; `None` while it is the default.
    pub updated_by: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Everyone the report is for, whether or not it is emailed to them.
pub async fn recipients(db: &DatabaseConnection) -> Result<Vec<Recipient>, DbErr> {
    let staff = globals::staff_holding(db, AUDIENCE).await?;
    let stored = store::preferences(db).await?;
    Ok(with_preferences(staff, stored))
}

/// On unless a stored answer says off.
fn with_preferences(
    staff: Vec<StaffAddress>,
    mut stored: HashMap<String, Preference>,
) -> Vec<Recipient> {
    staff
        .into_iter()
        .map(|staff| match stored.remove(&staff.email) {
            Some(answer) => Recipient {
                staff,
                enabled: answer.enabled,
                updated_by: answer.updated_by,
                updated_at: Some(answer.updated_at),
            },
            None => Recipient {
                staff,
                enabled: true,
                updated_by: None,
                updated_at: None,
            },
        })
        .collect()
}

/// Turn the email on or off for one recipient, on `by`'s say. `None` when
/// `email` is not someone the report is for — a preference is not stored for
/// an address that would never be mailed.
pub async fn set_enabled(
    db: &DatabaseConnection,
    email: &str,
    enabled: bool,
    by: &str,
) -> Result<Option<Recipient>, DbErr> {
    let target = store::normalize(email);
    let listed = recipients(db).await?;
    if !listed.iter().any(|r| r.staff.email == target) {
        return Ok(None);
    }
    store::set_wants_email(db, &target, enabled, by).await?;
    Ok(recipients(db)
        .await?
        .into_iter()
        .find(|r| r.staff.email == target))
}

/// The addresses that should have this report and do not yet.
pub async fn pending(
    db: &DatabaseConnection,
    report: &StoredReport,
) -> Result<Vec<StaffAddress>, DbErr> {
    let claimed = store::claimed(db, report.id).await?;
    Ok(still_owed(recipients(db).await?, &claimed))
}

fn still_owed(recipients: Vec<Recipient>, claimed: &HashSet<String>) -> Vec<StaffAddress> {
    recipients
        .into_iter()
        .filter(|r| r.enabled && !claimed.contains(&r.staff.email))
        .map(|r| r.staff)
        .collect()
}

/// Send the report to each of `readers`, within the pass's budget.
pub async fn deliver(
    db: &DatabaseConnection,
    report: &StoredReport,
    readers: Vec<StaffAddress>,
    mailer: &ReportMailer,
    console_url: Option<&str>,
) -> Result<Delivered, DbErr> {
    let started = Instant::now();
    let mut done = Delivered::default();
    for reader in readers {
        if started.elapsed() >= MAIL_BUDGET {
            break;
        }
        let slice = report.snapshot.scoped(&reader.scope);
        if slice.is_silent() {
            done.nothing_to_say += 1;
            continue;
        }
        if !store::claim_delivery(db, report.id, &reader.email).await? {
            continue;
        }
        match send(&slice, &reader.email, mailer, console_url).await {
            Ok(()) => {
                store::mark_sent(db, report.id, &reader.email).await?;
                done.sent += 1;
            }
            Err(error) => {
                tracing::warn!(
                    report_id = %report.id, %error,
                    "usage report not sent to one reader; retrying next pass"
                );
                store::release_delivery(db, report.id, &reader.email).await?;
                done.failed += 1;
            }
        }
    }
    Ok(done)
}

async fn send(
    slice: &super::model::Snapshot,
    to: &str,
    mailer: &ReportMailer,
    console_url: Option<&str>,
) -> Result<(), String> {
    let message = email::compose(slice, console_url).map_err(|e| e.to_string())?;
    match tokio::time::timeout(SEND_TIMEOUT, mailer.send(to, message)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".to_string()),
    }
}

/// Why a copy somebody asked for was not sent.
#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("No usage report has been written yet.")]
    NoReport,
    #[error("This deployment has no email sender configured.")]
    NoSender,
    #[error("The email could not be sent: {0}")]
    Send(String),
    #[error(transparent)]
    Db(#[from] DbErr),
}

/// Send the latest report to one reader now, outside the weekly pass and
/// whatever their email preference says — they asked. Not recorded as a
/// delivery: the Monday mail still goes out.
pub async fn send_copy(
    db: &DatabaseConnection,
    to: &str,
    scope: &Scope,
    console_url: Option<&str>,
) -> Result<DeliveryMode, CopyError> {
    let report = store::latest(db).await?.ok_or(CopyError::NoReport)?;
    let mailer = ReportMailer::for_request()
        .await
        .ok_or(CopyError::NoSender)?;
    send(&report.snapshot.scoped(scope), to, &mailer, console_url)
        .await
        .map_err(CopyError::Send)?;
    Ok(mailer.mode())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staff(emails: &[&str]) -> Vec<StaffAddress> {
        emails
            .iter()
            .map(|e| StaffAddress {
                email: e.to_string(),
                scope: Scope::All,
                role: None,
            })
            .collect()
    }

    fn turned_off(by: &str) -> Preference {
        Preference {
            enabled: false,
            updated_by: Some(by.to_string()),
            updated_at: Utc::now(),
        }
    }

    /// The mail links to a page behind `Action::PlatformOperate`. If the two
    /// ever name different people, someone is mailed a link they cannot open.
    #[test]
    fn the_audience_is_exactly_who_the_console_page_admits() {
        use oxy_authz::{Action, PlatformRole, PlatformStanding, PrincipalFacts, Resource, allows};
        for role in PlatformRole::ALL {
            let standing = PlatformStanding::from_role(role, Scope::All);
            let mailed = standing.holds(AUDIENCE);
            let facts = PrincipalFacts {
                platform: Some(standing),
                ..PrincipalFacts::default()
            };
            assert_eq!(
                allows(&facts, Action::PlatformOperate, &Resource::platform()),
                mailed,
                "{role:?}"
            );
        }
        let owner = PrincipalFacts {
            is_global_owner: true,
            ..PrincipalFacts::default()
        };
        assert!(allows(
            &owner,
            Action::PlatformOperate,
            &Resource::platform()
        ));
    }

    #[test]
    fn the_email_is_on_for_everyone_who_was_never_turned_off() {
        let stored = HashMap::from([("quiet@oxy.tech".to_string(), turned_off("root@oxy.tech"))]);
        let all = with_preferences(staff(&["root@oxy.tech", "quiet@oxy.tech"]), stored);
        assert!(all[0].enabled);
        assert_eq!(
            (all[0].updated_by.as_deref(), all[0].updated_at),
            (None, None)
        );
        assert!(!all[1].enabled);
        // Who turned it off travels with the answer.
        assert_eq!(all[1].updated_by.as_deref(), Some("root@oxy.tech"));
    }

    #[test]
    fn a_reader_who_is_turned_off_or_already_has_it_is_not_owed_it() {
        let stored = HashMap::from([("quiet@oxy.tech".to_string(), turned_off("quiet@oxy.tech"))]);
        let all = with_preferences(
            staff(&["root@oxy.tech", "quiet@oxy.tech", "done@oxy.tech"]),
            stored,
        );
        let claimed = HashSet::from(["done@oxy.tech".to_string()]);
        let owed = still_owed(all, &claimed);
        let emails: Vec<&str> = owed.iter().map(|s| s.email.as_str()).collect();
        assert_eq!(emails, ["root@oxy.tech"]);
    }
}
