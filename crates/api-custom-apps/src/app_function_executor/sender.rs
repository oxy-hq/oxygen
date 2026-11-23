//! Who a delivered `ctx.apps.send` runs as (custom apps architecture §6.1).
//!
//! A send runs as the person who sent it, as a synchronous `ctx.apps.call`
//! callee runs as the chain's person, and never with more than that person
//! holds. Three things make that so, and each is checked here, at the
//! delivery, when the function is about to run:
//!
//! 1. **Only a direct invoker.** The relay hands over an [`Invoker`] only when
//!    the host signed one (`app_events::provenance`, message domain v2), and
//!    the host signs one only for a send made by a run that itself ran as that
//!    person (`host::apps_send`). A send made from an event's delivery, a
//!    schedule or any system run carries none, whatever `actor` its chain
//!    carried, and runs as the system ([`person_for`] answers `None`).
//! 2. **The same credential, re-read now.** A token invoker is re-admitted by
//!    its id exactly as the live request path admits a presented token
//!    (`oxy_auth::token::store::resolve_stored`): its grants, its ceiling, its
//!    org blocks, a sandbox agent token's apps, its principal still active.
//!    A revoked, expired or narrowed-away token, or a deactivated person, is
//!    refused. The caller is then built from that credential with
//!    `Caller::from_user`, the live path's own constructor; a stored id is
//!    never handed a credential-less caller (`Caller::without_credential`),
//!    which would drop the ceiling and pick up the address's staff grants.
//!    A login session has no token to re-read: the person is re-read
//!    ([`session_caller`]).
//! 3. **The same door as a call.** The person must be allowed to open the
//!    receiving app in the delivery's environment: `Action::AppCall` through
//!    `oxy-authz` in production, `may_open_environment` elsewhere
//!    (`app_call_access::may_call_as_checked`), the check `ctx.apps.call` makes, which
//!    tells a refusal from a read that failed: only a refusal kills.
//!
//! A refusal kills the delivery with a reason starting [`SENDER_REFUSED`]: it
//! is delivered to nobody, and no retry is spent on it. A lookup that failed
//! this time fails the attempt, and the queue retries it.

use entity::{apps, users};
use oxy_custom_apps::events::OxyEvent;
use oxy_server_authz::Caller;
use oxy_shared::errors::OxyError;
use sea_orm::EntityTrait;
use uuid::Uuid;

use super::JobArgs;
use crate::app_events::deliveries::SENDER_REFUSED;
use crate::app_events::provenance::Invoker;
use crate::custom_apps_functions::host::app_call_access::may_call_as_checked;
use crate::custom_apps_functions::host::apps_op::Person;

/// Why a send's delivery does not run as its sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotRun {
    /// It never will: killed with this reason.
    Refused(String),
    /// A lookup failed this time: the attempt fails, the queue retries.
    Retry(String),
}

fn refused(why: impl std::fmt::Display) -> NotRun {
    NotRun::Refused(format!(
        "{SENDER_REFUSED}: {why}; it is delivered to nobody"
    ))
}

/// The person a send's delivery runs as: `None` (the system) when the relay
/// handed over no invoker, else the sender stood up again and allowed to open
/// the receiving app. See the module docs.
pub(super) async fn person_for(
    job: &JobArgs,
    envelope: &OxyEvent,
    invoker: Option<Invoker>,
) -> Result<Option<Person>, NotRun> {
    let Some(invoker) = invoker else {
        return Ok(None);
    };
    let user_id = envelope
        .actor
        .as_deref()
        .and_then(|actor| actor.parse::<Uuid>().ok())
        .ok_or_else(|| refused("the send names a sender but no person"))?;
    if invoker == Invoker::Session && session_lapsed(envelope.time, chrono::Utc::now()) {
        return Err(refused(
            "the login session it was sent on has ended: no login session outlives 30 days, \
             and this send is older",
        ));
    }
    let person = stand_up(&job.db, user_id, invoker).await?;
    let app = apps::Entity::find_by_id(job.app_id)
        .one(&job.db)
        .await
        .map_err(|e| NotRun::Retry(format!("could not read the receiving app: {e}")))?
        .ok_or_else(|| refused("the receiving app is gone"))?;
    let environment = &job.environment;
    let verdict = may_call_as_checked(
        &job.db,
        &person.caller,
        &app,
        environment,
        "custom_app_send_delivery",
    )
    .await;
    access_verdict(verdict, &app.slug, environment)?;
    Ok(Some(person))
}

/// What the delivery does with the access check's answer: run on `true`, kill
/// on `false` (a real refusal), retry on `Err` (a read that failed this time,
/// which is no refusal).
fn access_verdict(
    verdict: Result<bool, sea_orm::DbErr>,
    app: &str,
    environment: &oxy_app_core::custom_app_environment::AppEnvironment,
) -> Result<(), NotRun> {
    match verdict {
        Ok(true) => Ok(()),
        Ok(false) => Err(refused(format!(
            "the person who sent it may not open `{app}` in {environment}"
        ))),
        Err(e) => Err(NotRun::Retry(format!(
            "could not decide whether the sender may open `{app}`: {e}"
        ))),
    }
}

/// Whether a login session that sent an event at `sent` has certainly ended by
/// `now`. A login session is a signed token with no server-side record: it
/// ends at its expiry, and only deactivating its person (checked at the
/// stand-up) ends it sooner, on the live path as here. Its exact expiry is not
/// carried, but no login session outlives `SESSION_TTL_SECS` from its mint,
/// which was no later than the send. So a delivery, or a dead letter replayed
/// weeks later, never runs as a person past the latest end their session could
/// have had.
fn session_lapsed(sent: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> bool {
    let ttl = chrono::Duration::seconds(oxy_app_core::session::SESSION_TTL_SECS);
    now.signed_duration_since(sent) >= ttl
}

/// `user_id` under the credential `invoker` names, as the live request path
/// would build them now, or why not.
async fn stand_up(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    invoker: Invoker,
) -> Result<Person, NotRun> {
    let user = users::Entity::find_by_id(user_id)
        .one(db)
        .await
        .map_err(|e| NotRun::Retry(format!("could not read the sender: {e}")))?
        .ok_or_else(|| refused("the person who sent it no longer exists"))?;
    if user.status != users::UserStatus::Active {
        return Err(refused("the person who sent it is no longer active"));
    }
    let user = oxy_auth::types::AuthenticatedUser::from(user);
    let caller = match invoker {
        Invoker::Session => session_caller(&user),
        Invoker::Token(token_id) => token_caller(db, &user, token_id).await?,
    };
    Ok(Person {
        caller,
        user_id: user.id,
        email: user.email,
        name: Some(user.name),
        picture: user.picture,
        relayed_session: invoker == Invoker::Session,
    })
}

/// A login session's caller, for its person re-read now: what `Caller::of`
/// builds for a request on a login session, which is what the sending run
/// had. A login session carries no token, so there is no ceiling to keep, and
/// nothing here widens what the person held: every standing it can carry is
/// re-read at the delivery, so one revoked since the send is gone.
///
/// That includes Oxy staff. Their reach into an org's app is platform standing
/// on the live path too (the staff term of `user_can_access_app`, and
/// `may_open_non_production` off production), never an assume-role session:
/// it is read through `globals`' 60 s grant cache at the delivery as at a
/// request. The tenant reach an assume session gives (a synthesized Owner, an
/// assumed partner) is read only from sessions live now, by the credential's
/// own binding, and a function run drops a synthesized Owner anyway.
fn session_caller(user: &oxy_auth::types::AuthenticatedUser) -> Caller {
    Caller::from_user(user)
}

/// `user` under the stored token `token_id`, re-admitted now with its
/// ceiling, or refused when it no longer admits or is not theirs.
async fn token_caller(
    db: &sea_orm::DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
    token_id: Uuid,
) -> Result<Caller, NotRun> {
    let resolved = oxy_auth::token::store::resolve_stored(db, token_id)
        .await
        .map_err(readmit_failure)?;
    if resolved.identity.user_id != Some(user.id) {
        return Err(refused(
            "the credential it was sent with is not the sender's",
        ));
    }
    let user = user.clone().with_credential(Some(resolved.credential));
    Ok(Caller::from_user(&user))
}

/// What a failed re-admission means for the delivery: a read that failed
/// (`DBError`, which `resolve_stored` gives every failed lookup, the sender's
/// own row included) is retried, and anything else is the credential no
/// longer admitting: killed.
fn readmit_failure(err: OxyError) -> NotRun {
    match err {
        OxyError::DBError(e) => {
            NotRun::Retry(format!("could not read the sender's credential: {e}"))
        }
        _ => refused(
            "the credential it was sent with no longer admits (revoked, expired, narrowed \
             away, or its owner deactivated)",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{NotRun, access_verdict, readmit_failure, session_lapsed};
    use oxy_app_core::custom_app_environment::AppEnvironment;
    use oxy_shared::errors::OxyError;

    /// F1: a failed read is retried, never taken for a refusal; only a real
    /// refusal kills the delivery.
    #[test]
    fn a_failed_access_read_is_retried_and_only_a_refusal_kills() {
        let env = AppEnvironment::Production;
        assert_eq!(access_verdict(Ok(true), "b", &env), Ok(()));
        assert!(matches!(
            access_verdict(Ok(false), "b", &env),
            Err(NotRun::Refused(reason)) if reason.starts_with("sender_refused")
        ));
        assert!(matches!(
            access_verdict(
                Err(sea_orm::DbErr::Custom("pool timed out".into())),
                "b",
                &env
            ),
            Err(NotRun::Retry(_))
        ));
    }

    /// A read that failed while re-admitting the credential (the token's
    /// rows, its owner's) is retried; a credential that no longer admits is
    /// killed. `key_owner` giving a failed `users` read as `DBError` is
    /// pinned in `oxy_auth::api_key_infra`.
    #[test]
    fn a_failed_read_re_admitting_a_token_is_retried_and_a_refusal_kills() {
        assert!(matches!(
            readmit_failure(OxyError::DBError(
                "api key owner lookup: pool timed out".into()
            )),
            NotRun::Retry(_)
        ));
        for refusal in [
            OxyError::AuthenticationError("Invalid API key".into()),
            OxyError::AuthenticationError("User not found".into()),
            OxyError::AuthenticationError("This account is no longer active".into()),
        ] {
            assert!(matches!(readmit_failure(refusal), NotRun::Refused(_)));
        }
    }

    use chrono::{Duration, TimeZone, Utc};

    /// A send delivered within a login session's longest life may still run
    /// as its person; one older than that never does, since its session had
    /// certainly ended.
    #[test]
    fn a_session_send_older_than_any_session_has_lapsed() {
        let sent = Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap();
        assert!(!session_lapsed(sent, sent));
        assert!(!session_lapsed(sent, sent + Duration::days(29)));
        assert!(session_lapsed(sent, sent + Duration::days(30)));
        assert!(session_lapsed(sent, sent + Duration::days(45)));
    }
}
