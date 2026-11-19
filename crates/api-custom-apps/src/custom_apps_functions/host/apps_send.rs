//! `ctx.apps.send` on the host: the same call as `ctx.apps.call`, queued
//! instead of awaited. It is written as a directed Phase 1 event in the org's
//! outbox, through the writer the app publishes as (`events_op`), so it needs
//! the org's OLTP store as any publish does, and it is relayed, delivered,
//! retried and dead-lettered as an event is — to the one function it names.
//!
//! The row's name is [`send_name`] (`call:<app>:<function>:<caller function>`),
//! which the relay reads as a directed event: it resolves the callee in the
//! environment the row was published in, and stores the event with a relay error
//! (delivered to nobody, and visible in the events API) when the app, function
//! or build is missing or the function is not callable. There is no schema; the
//! body is JSON under the event payload limit. The callee receives the body as
//! its request, with `ctx.caller` set, as a call would give it.
//!
//! The answer is `{ eventId }`, the id of the event to look up. The key is the
//! caller's `key` when given (a repeat of one is the same send), else a new one
//! per send; `delaySeconds` is 0 to 86 400 as for any event.
//!
//! **Who it is delivered as.** A run that runs as a person (a route call, or a
//! call or send from one, which run as that person) sends as them: the row
//! records the credential they ran under ([`send_invoker`]: their login
//! session, or the stored token they arrived with) and the host signs it
//! beside the row (`app_events::provenance`, message domain v2). The delivery
//! re-reads that credential, asks the same `Action::AppCall` door a call asks,
//! and runs the function as them or refuses it (`app_function_executor::
//! sender`). A run that runs as the system (an event's delivery, a schedule,
//! a send delivered as the system) records none, whatever `actor` its chain
//! carried, so its sends run as the system.

use oxy_custom_apps::calls::send_name;
use oxy_custom_apps::events::{MAX_KEY_LEN, MAX_PAYLOAD_BYTES, jsonb_text_len};
use oxy_oltp::events::PUBLISH_WITH_INVOKER_SQL;
use serde_json::{Value, json};

use super::apps_op::{Person, Target};
use super::events_op::{Checked, delay_seconds, holds_nul};
use super::*;
use crate::app_events::provenance::Invoker;

const KEY_INVALID: &str = "AppCallRefused: `key` must be 1–200 characters without U+0000";

impl ProjectFunctionHost {
    /// `ctx.apps.send`: the checked call, queued. See the module docs.
    pub(super) async fn send(
        &self,
        host_op: HostOp,
        target: Target,
        payload: &Value,
    ) -> Result<Value, String> {
        let checked = self.check_send(&target, payload).await?;
        let invoker = send_invoker(self.person.as_ref(), self.chain().actor.as_deref());
        delay_within_credential(
            invoker.as_ref(),
            self.person.as_ref(),
            checked.delay_seconds,
            chrono::Utc::now(),
        )?;
        let decided = self.policy.decide(host_op);
        let outbox = self.admit_publish(host_op, decided, &checked.name).await?;
        let conn = self.publish_writer(host_op, &outbox).await?;
        let (id, params) = self.publish_params_with_invoker(&checked, &conn.role, invoker);
        self.publish_alone(&outbox, &conn, PUBLISH_WITH_INVOKER_SQL, &params)
            .await?;
        self.after_publish_commit().await;
        Ok(json!({ "eventId": id.to_string() }))
    }

    /// The checks a send adds to a call's: the callee app exists, the key and
    /// delay are in bounds and the body fits an event.
    async fn check_send(&self, target: &Target, payload: &Value) -> Result<Checked, String> {
        let app = self.find_target_app(&target.app).await?;
        // A person is asked now, and again at delivery (`sender`): the
        // credential or the app's access may change between the two.
        self.authorize_target(&app).await?;
        let key = match payload.get("key") {
            None | Some(Value::Null) => uuid::Uuid::new_v4().to_string(),
            Some(Value::String(key))
                if (1..=MAX_KEY_LEN).contains(&key.chars().count()) && !key.contains('\0') =>
            {
                key.clone()
            }
            _ => return Err(KEY_INVALID.to_string()),
        };
        let delay_seconds = delay_seconds(payload.get("delaySeconds"))
            .map_err(|e| e.replacen("EventDelayInvalid", "AppCallRefused", 1))?;
        let bytes = jsonb_text_len(&target.body);
        if bytes > MAX_PAYLOAD_BYTES || holds_nul(&target.body) {
            return Err(format!(
                "AppCallRefused: a send's body is held to an event's: {MAX_PAYLOAD_BYTES} bytes \
                 of JSON without U+0000 (this one is {bytes} bytes)"
            ));
        }
        Ok(Checked {
            name: send_name(&target.app, &target.function, &self.identity.function_name),
            key,
            data: target.body.clone(),
            delay_seconds,
            data_bytes: bytes,
            revision: None,
        })
    }
}

/// Who a send from a run is sent as: the person the run runs as, by the
/// credential they arrived with, when the run runs as that person — its
/// chain's actor is them. `None` for a run that runs as the system (an event's
/// delivery, a schedule), whatever actor its chain carried; for a person
/// whose credential cannot be re-read outside this request (a configured
/// service, which has no stored row); and for a person stood up from a login
/// session at a send's delivery, whose session's end is not known
/// (`Person::relayed_session`): their sends run as the system.
fn send_invoker(person: Option<&Person>, chain_actor: Option<&str>) -> Option<Invoker> {
    let person = person?;
    if person.relayed_session || chain_actor != Some(person.user_id.to_string().as_str()) {
        return None;
    }
    if person.caller.is_login_session() {
        return Some(Invoker::Session);
    }
    person.caller.stored_token_id().map(Invoker::Token)
}

/// Refuse a send whose delay outlives the credential it is sent under. The
/// delivery re-admits a token invoker (`app_function_executor::sender`), and a
/// token that has expired by then is refused for good: the send would be
/// accepted now and killed later with no word to its author. A login session,
/// a token that never expires and a send as the system carry no expiry here.
fn delay_within_credential(
    invoker: Option<&Invoker>,
    person: Option<&Person>,
    delay_seconds: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let (Some(Invoker::Token(_)), Some(person)) = (invoker, person) else {
        return Ok(());
    };
    let Some(expires_at) = person.caller.token_expires_at() else {
        return Ok(());
    };
    if now + chrono::Duration::seconds(delay_seconds) < expires_at {
        return Ok(());
    }
    Err(format!(
        "AppCallRefused: `delaySeconds` of {delay_seconds} runs past the credential this run is \
         on, which expires at {}, before the send would be delivered: shorten the delay",
        expires_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_auth::token::{CredentialContext, StoredKind};
    use oxy_auth::types::AuthenticatedUser;

    const TOKEN: Uuid = Uuid::from_u128(0x70);

    fn user() -> AuthenticatedUser {
        AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        }
    }

    fn credential(kind: StoredKind) -> CredentialContext {
        CredentialContext {
            token_id: TOKEN,
            kind,
            principal_user_id: Uuid::from_u128(1),
            all_access: true,
            platform: false,
            partner: false,
            name: "t".into(),
            display_prefix: "oxy_pat_Ab3x".into(),
            grants: Vec::new(),
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
            blocked_orgs: Vec::new(),
            service_account: None,
            app_access: None,
            expires_at: None,
        }
    }

    fn person(credential: Option<CredentialContext>) -> Person {
        let user = user().with_credential(credential);
        Person {
            caller: oxy_server_authz::Caller::from_user(&user),
            user_id: user.id,
            email: user.email,
            name: Some(user.name),
            picture: None,
            relayed_session: false,
        }
    }

    fn ada() -> String {
        Uuid::from_u128(1).to_string()
    }

    /// A run as a person sends as them, under the credential they arrived
    /// with: a login session, or the stored token by its id.
    #[test]
    fn a_run_as_a_person_sends_under_their_credential() {
        assert_eq!(
            send_invoker(Some(&person(None)), Some(&ada())),
            Some(Invoker::Session)
        );
        for kind in [StoredKind::Personal, StoredKind::SandboxAgent] {
            assert_eq!(
                send_invoker(Some(&person(Some(credential(kind)))), Some(&ada())),
                Some(Invoker::Token(TOKEN)),
                "{kind:?}"
            );
        }
    }

    /// The review's first finding: an actor the chain merely carried (an
    /// event's delivery, a schedule: no person) is never an invoker.
    #[test]
    fn an_inherited_actor_is_never_an_invoker() {
        assert_eq!(send_invoker(None, Some(&ada())), None);
        assert_eq!(send_invoker(None, None), None);
    }

    /// F2: a person stood up from a send's login session sends as the system,
    /// so a session is never carried past the hop it was checked at; one stood
    /// up from a token sends under it again, re-admitted at the next hop.
    #[test]
    fn a_relayed_session_sends_as_the_system_and_a_relayed_token_does_not() {
        let relayed = |credential: Option<CredentialContext>| Person {
            relayed_session: credential.is_none(),
            ..person(credential)
        };
        assert_eq!(send_invoker(Some(&relayed(None)), Some(&ada())), None);
        assert_eq!(
            send_invoker(
                Some(&relayed(Some(credential(StoredKind::Personal)))),
                Some(&ada())
            ),
            Some(Invoker::Token(TOKEN))
        );
    }

    /// A person whose chain names someone else, or no one, is not sending as
    /// themselves: no invoker.
    #[test]
    fn a_person_and_a_chain_that_disagree_send_as_the_system() {
        let other = Uuid::from_u128(2).to_string();
        assert_eq!(send_invoker(Some(&person(None)), Some(&other)), None);
        assert_eq!(send_invoker(Some(&person(None)), None), None);
    }

    /// A delayed send from a short-lived token is refused when the delay
    /// reaches the token's expiry, and allowed when it falls short of it.
    #[test]
    fn a_delay_that_outlives_the_token_is_refused() {
        let now = chrono::Utc::now();
        let mut credential = credential(StoredKind::SandboxAgent);
        credential.expires_at = Some(now + chrono::Duration::seconds(600));
        let person = person(Some(credential));
        let invoker = Some(Invoker::Token(TOKEN));
        let check = |delay| delay_within_credential(invoker.as_ref(), Some(&person), delay, now);
        assert_eq!(check(0), Ok(()));
        assert_eq!(check(599), Ok(()));
        for delay in [600, 3600] {
            let refused = check(delay).unwrap_err();
            assert!(refused.starts_with("AppCallRefused:"), "{refused}");
            assert!(
                refused.contains(&format!("`delaySeconds` of {delay}")),
                "{refused}"
            );
            assert!(refused.contains("expires at"), "{refused}");
        }
    }

    /// A login session, a token that never expires and a send as the system
    /// have no expiry to outlive, whatever the delay.
    #[test]
    fn a_delay_is_free_for_a_session_a_token_without_expiry_and_the_system() {
        let now = chrono::Utc::now();
        let day = 86_400;
        let session = person(None);
        assert_eq!(
            delay_within_credential(Some(&Invoker::Session), Some(&session), day, now),
            Ok(())
        );
        let forever = person(Some(credential(StoredKind::Personal)));
        assert_eq!(
            delay_within_credential(Some(&Invoker::Token(TOKEN)), Some(&forever), day, now),
            Ok(())
        );
        assert_eq!(delay_within_credential(None, None, day, now), Ok(()));
        // A credential that expires, but a run with no invoker (the system).
        let mut short = credential(StoredKind::Personal);
        short.expires_at = Some(now + chrono::Duration::seconds(1));
        let short = person(Some(short));
        assert_eq!(
            delay_within_credential(None, Some(&short), day, now),
            Ok(())
        );
    }
}
