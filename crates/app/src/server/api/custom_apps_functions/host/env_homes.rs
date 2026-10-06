//! Where an op with an **isolated home** runs outside production (Phase 5a;
//! environments design §4.2): the storage silo, invoker-only email and the
//! environment's secret path.
//!
//! As with `env_guard`, the decision is `EnvPolicy::decide`, made once per op
//! and never re-derived here; this turns an `Isolate(target)` into the
//! resource the op works on. In production every helper answers the
//! production resource, so a production host behaves exactly as before.

use super::super::env_policy::{self, Decision, HostOp, Target};
use super::env_guard::HeldTarget;
use super::*;
use crate::emails::app_emailer::{AppEmailer, EmailSendInput};
use crate::server::api::custom_apps_storage::Silo;

/// `Refuse` fix for `ctx.secrets.set` on a key this run read from another
/// environment: production's through the `shared` fallback, or — in a sandbox
/// — staging's.
const FALLBACK_WRITE_FIX: &str = "this invocation read the key from another environment \
     through a fallback (staging's value, or production's `shared` one), and writing it here \
     would fork that environment's grant (a rotated token voids the one it holds). Set this \
     environment's own value of the key instead";

/// `Refuse` fix for an isolated email with nobody to deliver it to.
const NO_INVOKER_FIX: &str = "mail outside production goes only to the invoking user, and \
     this invocation has no verified human caller with an email address";

/// The verified human caller's address, or `None` — a system run (a schedule
/// executes as the org owner, `run_scheduled_function`) has no invoker, and
/// falling back to the owner would mail a real person nobody asked to test
/// with. Only `ctx.user`'s verified email counts (`user_email` is set for a
/// human route caller alone).
fn invoker_email(identity: &super::super::data_audit::InvocationIdentity) -> Option<String> {
    identity.user_id?;
    identity
        .user_email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

impl ProjectFunctionHost {
    /// Where `op` runs here: `None` as asked (production, or a read of
    /// production data), `Some(target)` for its environment's isolated home.
    /// A held or refused op is noted and its error returned, as `admit_op`
    /// does.
    pub(super) async fn route_op(
        &self,
        op: HostOp,
        target: HeldTarget<'_>,
    ) -> Result<Option<Target>, String> {
        match self.policy.decide(op) {
            Decision::Isolate(home) => Ok(Some(home)),
            _ => self.admit_op(op, target).await.map(|()| None),
        }
    }

    /// The silo a `ctx.storage` op works in: production's, or — when the
    /// policy isolates it — the environment's sibling.
    pub(super) async fn storage_silo(
        &self,
        op: HostOp,
        target: HeldTarget<'_>,
    ) -> Result<Silo, String> {
        // Notes a held or refused op and returns its error; the silo itself is
        // the policy's answer (`EnvPolicy::silo_for`), the one the
        // differential test resolves too.
        self.route_op(op, target).await?;
        self.policy
            .silo_for(op, self.app_id)
            .ok_or_else(|| self.misrouted(op))
    }

    /// The storage limit a write of `incoming` bytes into `silo` answers to:
    /// production's org quota, or — an environment silo — that silo's own soft
    /// cap, never the org's, so a staging loop cannot pause production's
    /// writes. The silo is listed once per invocation (`SiloMeter`).
    pub(super) async fn storage_limit(&self, silo: &Silo, incoming: u64) -> Result<(), String> {
        use crate::server::api::custom_apps_storage::quota;
        let checked = if silo.is_production() {
            quota::check_write_allowed(&self.db, self.org_id, incoming).await
        } else {
            self.environment_meter.admit(silo, incoming).await
        };
        checked.map_err(|e| e.to_string())
    }

    /// A `copy` into an environment silo already past its cap is
    /// refused. Production's `copy` is not gated, as before.
    pub(super) async fn environment_copy_limit(&self, silo: &Silo) -> Result<(), String> {
        if silo.is_production() {
            return Ok(());
        }
        self.storage_limit(silo, 0).await
    }

    /// Send `parsed` as production would, or — isolated — to the invoking
    /// user only, subject prefixed `[<env>]`, reporting back the recipients
    /// the call named and did not reach. `OXY_APP_EMAIL_LOCAL_TEST` previews
    /// either exactly as before: only the message changed.
    pub(super) async fn deliver_email(
        &self,
        parsed: EmailSendInput,
    ) -> Result<serde_json::Value, String> {
        let emailer = AppEmailer::from_env(self.app_name.clone());
        let target = ("email", "", "SEND", "");
        match self.route_op(HostOp::EmailSend, target).await? {
            None => emailer.send(parsed).await,
            Some(Target::InvokerEmail) if !self.policy.is_production() => {
                let environment = self.policy.environment().name();
                let Some(invoker) = self.invoker_email().await else {
                    self.note_held(HostOp::EmailSend, target).await;
                    return Err(env_policy::refused_message(
                        HostOp::EmailSend,
                        self.policy.environment(),
                        NO_INVOKER_FIX,
                    ));
                };
                let (message, ignored) = parsed.redirected_to(&invoker, &environment)?;
                tracing::info!(
                    %environment,
                    ignored = ignored.len(),
                    "ctx.email.send delivered to the invoking user only"
                );
                let mut sent = emailer.send(message).await?;
                if let Some(result) = sent.as_object_mut() {
                    result.insert("environment".into(), environment.into());
                    result.insert("deliveredTo".into(), serde_json::json!([invoker]));
                    result.insert("ignoredRecipients".into(), ignored.to_json());
                }
                Ok(sent)
            }
            Some(_) => Err(self.misrouted(HostOp::EmailSend)),
        }
    }

    /// Who "the invoking user" is for mail: the verified human caller of a
    /// route, and nobody else ([`invoker_email`]).
    async fn invoker_email(&self) -> Option<String> {
        invoker_email(&self.identity)
    }

    /// Write `key` for `ctx.secrets.set`: production's `apps/<id>/<KEY>`, or —
    /// isolated — the environment's `apps/<id>/<env>/<KEY>`. Refused for a key
    /// this run read from another environment through a fallback.
    pub(super) async fn set_secret(
        &self,
        key: &str,
        value: &str,
        target: HeldTarget<'_>,
    ) -> Result<(), String> {
        let environment = match self.route_op(HostOp::SecretsSet, target).await? {
            None => None,
            Some(Target::EnvSecrets) => {
                if self.policy.read_through_fallback(key) {
                    self.note_held(HostOp::SecretsSet, target).await;
                    return Err(env_policy::refused_message(
                        HostOp::SecretsSet,
                        self.policy.environment(),
                        FALLBACK_WRITE_FIX,
                    ));
                }
                // A production policy has no environment path: its segment
                // is `None`, which would name production's key.
                let Some(segment) = self.policy.secret_segment() else {
                    return Err(self.misrouted(HostOp::SecretsSet));
                };
                Some(segment)
            }
            Some(_) => return Err(self.misrouted(HostOp::SecretsSet)),
        };
        SecretManagerService::new(self.project_id)
            .set_app_secret_in(
                &self.db,
                self.app_id,
                environment.as_deref(),
                key,
                value,
                self.actor,
            )
            .await
            // `ctx.secrets.set` resolves to nothing either way: whether the write
            // created or rotated the key only sets the HTTP route's status code.
            .map(|_write| ())
            .map_err(|e| format!("ctx.secrets.set failed: {e}"))
    }

    /// An op whose policy names a home other than its own: refuse, never fall
    /// through to production.
    fn misrouted(&self, op: HostOp) -> String {
        env_policy::refused_message(op, self.policy.environment(), env_policy::MISROUTED_FIX)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::data_audit::InvocationIdentity;
    use super::invoker_email;
    use uuid::Uuid;

    fn identity(user_id: Option<Uuid>, email: Option<&str>) -> InvocationIdentity {
        InvocationIdentity {
            invocation_id: Uuid::nil(),
            function_name: "f".to_string(),
            mode: "route".to_string(),
            request_id: None,
            app_slug: "a".to_string(),
            user_id,
            user_email: email.map(str::to_string),
            credential_token_id: None,
        }
    }

    /// No org-owner fallback: a run with no verified human caller has no
    /// invoker, so its staging mail is refused rather than sent to the owner.
    #[test]
    fn only_a_verified_human_caller_is_an_invoker() {
        let human = Some(Uuid::from_u128(1));
        assert_eq!(
            invoker_email(&identity(human, Some(" staff@oxy.tech "))).as_deref(),
            Some("staff@oxy.tech")
        );
        assert_eq!(invoker_email(&identity(None, None)), None, "a system run");
        assert_eq!(invoker_email(&identity(None, Some("owner@x.com"))), None);
        assert_eq!(invoker_email(&identity(human, Some("  "))), None);
        assert_eq!(invoker_email(&identity(human, None)), None, "a PIN worker");
    }
}
