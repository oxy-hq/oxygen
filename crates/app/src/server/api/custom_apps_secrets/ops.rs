//! The per-key operations behind every mount of the secrets surface — set,
//! delete, reveal — on one environment's path (`scope`).
//!
//! **Every key is a bare key** ([`bare_key`]), validated like any secret name:
//! one path segment, no `/`. Without it a path parameter of `staging%2FKEY` on
//! a production request would address `apps/<id>/staging/KEY` — another
//! environment's row — past the `AppNonProduction` gate
//! (`environment::authorize`), and be audited as production.
//!
//! **Outside production every write is audited** with its environment
//! (`custom_app.secret.set` / `.deleted`), as `reveal` is in every
//! environment: a staging value is set by staff, and the row says who.

use axum::Json;
use axum::http::StatusCode;
use entity::apps;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::audit;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

use super::{Failure, RevealResponse, SetSecretRequest, scope};

/// Whether a submitted value would land as the empty string.
///
/// Split out and named so the trim is pinned by a test rather than by a reader
/// noticing it — the bug this replaced was a bare `is_empty()`, which looks
/// correct until you read `sanitize_secret_value` two crates away.
pub(super) fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

/// `raw`, trimmed, when it is a bare key: a valid secret name, so one path
/// segment of `apps/<id>/[<env>/]<KEY>` and never a path into another
/// environment's. A 400 otherwise.
pub(super) fn bare_key(raw: &str) -> Result<&str, Failure> {
    let key = raw.trim();
    SecretManagerService::validate_secret_name(key)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid key {key:?}: {e}")))?;
    Ok(key)
}

/// Audit a staff write of a non-production secret. Production's writes are
/// logged as before; this row exists for the environment only staff reach.
async fn audit_write(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    action: &'static str,
    key: &str,
    actor: &AuthenticatedUser,
) {
    if *environment == AppEnvironment::Production {
        return;
    }
    audit::record_best_effort(
        db,
        audit::AuditEntry::new(actor.label().to_string(), action)
            .actor(actor.id, audit::ActorType::User)
            .org(app.org_id)
            .workspace(app.project_id)
            .target(
                "custom_app_secret",
                scope::secret_name(app.id, environment, key),
                format!("{}/{key}", app.slug),
            )
            .environment(environment.name()),
    )
    .await;
}

pub(super) async fn set(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    body: SetSecretRequest,
    actor: &AuthenticatedUser,
) -> Result<StatusCode, Failure> {
    let key = bare_key(&body.key)?;
    // `trim()`, not `is_empty()`. An empty value resolves to an empty
    // `ctx.env.KEY`, which reads as "configured" everywhere downstream while
    // behaving like unset — the `usable_secret` / `timingSafeEqual` footguns
    // both start here, and for a `webhook.secretVar` it means verifying an HMAC
    // against an empty key instead of answering 401 with a nameable cause.
    //
    // A whitespace-only value is that same state wearing a disguise:
    // `sanitize_secret_value` tests emptiness BEFORE it trims and then stores
    // the trimmed string, so `" "` clears both its check and a bare
    // `is_empty()` here and lands as `""`. Deleting is how you unset.
    if is_blank(&body.value) {
        return Err((
            StatusCode::BAD_REQUEST,
            "value must not be empty or whitespace — delete the secret instead of blanking it"
                .to_string(),
        ));
    }

    SecretManagerService::new(app.project_id)
        .set_app_secret_in(
            db,
            app.id,
            scope::environment_segment(environment).as_deref(),
            key,
            &body.value,
            actor.id,
        )
        .await
        .map_err(|e| {
            // `set_app_secret` validates the caller's key against the name
            // charset; that's a request problem, not a server one.
            tracing::warn!(app_id = %app.id, "setting app secret failed: {e}");
            (StatusCode::BAD_REQUEST, e.to_string())
        })?;

    tracing::info!(
        app_id = %app.id,
        secret.key = %key,
        %environment,
        actor = %actor.id,
        "app secret set"
    );
    audit_write(db, app, environment, "custom_app.secret.set", key, actor).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn delete(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    key: &str,
    actor: &AuthenticatedUser,
) -> Result<StatusCode, Failure> {
    // Trimmed, like `set` — otherwise `DELETE …/secrets/%20SIG` 404s on a key
    // the sibling POST would have normalised to `SIG`.
    let key = bare_key(key)?;
    let name = scope::secret_name(app.id, environment, key);
    SecretManagerService::new(app.project_id)
        .delete_secret(db, &name)
        .await
        .map_err(|e| {
            tracing::warn!(app_id = %app.id, "deleting app secret failed: {e}");
            // `delete_secret` distinguishes "no such row" from a database
            // failure by variant; collapsing both to 404 would report an
            // outage as a missing key.
            let code = match e {
                OxyError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::NOT_FOUND,
            };
            (code, e.to_string())
        })?;
    tracing::info!(
        app_id = %app.id,
        secret.key = %key,
        %environment,
        actor = %actor.label(),
        "app secret deleted"
    );
    audit_write(
        db,
        app,
        environment,
        "custom_app.secret.deleted",
        key,
        actor,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `SecretManagerService::get_secret` owns its own connection (and a 300s
/// decrypt cache); `db` is for the audit row.
pub(super) async fn reveal(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    key: &str,
    actor: &AuthenticatedUser,
) -> Result<Json<RevealResponse>, Failure> {
    // Parity with project secrets, which are already revealable by id on the
    // existing route — an app-scoped one is not a different kind of secret, and
    // pretending otherwise would just push people back to the raw table.
    let key = bare_key(key)?;
    let name = scope::secret_name(app.id, environment, key);
    let value = SecretManagerService::new(app.project_id)
        .get_secret(&name)
        .await
        .ok_or((StatusCode::NOT_FOUND, "secret not found".to_string()))?;

    // The one action here that hands a human plaintext tenant secret material,
    // reachable by any in-scope App Operator — so it is the one worth a durable
    // record rather than only a log line. Best-effort: failing to write the
    // audit row must not fail the read the operator is entitled to, and the
    // helper logs its own failure.
    audit::record_best_effort(
        db,
        audit::AuditEntry::new(actor.label().to_string(), "custom_app.secret.revealed")
            .actor(actor.id, audit::ActorType::User)
            .org(app.org_id)
            .workspace(app.project_id)
            .target(
                "custom_app_secret",
                name.clone(),
                format!("{}/{key}", app.slug),
            )
            .environment(environment.name()),
    )
    .await;

    tracing::info!(
        app_id = %app.id,
        secret.key = %key,
        actor = %actor.label(),
        "app secret revealed"
    );
    Ok(Json(RevealResponse {
        key: key.to_string(),
        value,
    }))
}

/// Delete every secret of `environment` of the app — everything under
/// `apps/<app_id>/<env>/` — and answer how many went. A sandbox's teardown
/// calls it (`custom_apps_sandboxes::teardown`); idempotent, so a retry after
/// a partial run removes the rest.
///
/// **Only a sandbox's secrets are removed here.** Production is refused,
/// deleting nothing: its prefix is `apps/<app_id>/`, which every
/// environment's secrets sit under, so "all of production's" would take the
/// app's whole store. Staging is refused too: nothing tears staging down.
/// The guard is this function's own, whatever its caller checked.
pub(crate) async fn delete_environment_secrets(
    db: &DatabaseConnection,
    project_id: uuid::Uuid,
    app_id: uuid::Uuid,
    environment: &AppEnvironment,
) -> Result<usize, OxyError> {
    let Some(prefix) = environment_prefix(app_id, environment) else {
        return Err(OxyError::SecretManager(format!(
            "refusing to delete every secret of app {app_id}'s {environment} environment: only \
             a sandbox's are removed together"
        )));
    };
    let manager = SecretManagerService::new(project_id);
    let names: Vec<String> = stored_names(&manager, db)
        .await?
        .into_iter()
        .filter(|name| name.starts_with(&prefix))
        .collect();
    let deleted = delete_named_secrets(db, project_id, &names).await?;
    tracing::info!(%app_id, %environment, deleted, "environment secrets deleted");
    Ok(deleted)
}

async fn stored_names(
    manager: &SecretManagerService,
    db: &DatabaseConnection,
) -> Result<Vec<String>, OxyError> {
    Ok(manager
        .list_secrets(db)
        .await?
        .into_iter()
        .map(|secret| secret.name)
        .collect())
}

/// Delete the stored secrets `names` of the project; how many this call
/// removed. **A name that is not stored is already deleted**, not a failure:
/// a concurrent or repeated teardown lists a name and then finds the other
/// run removed it. Any other failure — and a name that failed to delete and
/// is still stored — is the error.
///
/// Public so a test can hand it a name that is gone; the teardown reaches it
/// only through [`delete_environment_secrets`], which names one sandbox's.
pub async fn delete_named_secrets(
    db: &DatabaseConnection,
    project_id: uuid::Uuid,
    names: &[String],
) -> Result<usize, OxyError> {
    let manager = SecretManagerService::new(project_id);
    let mut deleted = 0;
    for name in names {
        match manager.delete_secret(db, name).await {
            Ok(()) => deleted += 1,
            Err(e) => {
                if stored_names(&manager, db).await?.contains(name) {
                    return Err(e);
                }
            }
        }
    }
    Ok(deleted)
}

/// The prefix [`delete_environment_secrets`] removes under: a sandbox's.
/// `None` for production, whose prefix holds every other environment's, and
/// for staging, which is never torn down.
fn environment_prefix(app_id: uuid::Uuid, environment: &AppEnvironment) -> Option<String> {
    matches!(environment, AppEnvironment::Dev { .. })
        .then(|| scope::secret_prefix(app_id, environment))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a sandbox has a prefix to delete under. Production has none — its
    /// `apps/<id>/` is the parent of every environment's — and neither has
    /// staging, which nothing tears down. A sandbox's ends in a slash, so it
    /// never matches a longer handle's.
    #[test]
    fn only_a_sandbox_has_a_prefix_to_delete_under() {
        let app = uuid::Uuid::from_u128(7);
        assert_eq!(environment_prefix(app, &AppEnvironment::Production), None);
        assert_eq!(environment_prefix(app, &AppEnvironment::Staging), None);
        let sandbox = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        let prefix = environment_prefix(app, &sandbox).expect("a sandbox");
        assert_eq!(prefix, format!("apps/{app}/dev-a1/"));
        assert!(!format!("apps/{app}/dev-a1-b/K").starts_with(&prefix));
        assert!(!format!("apps/{app}/K").starts_with(&prefix));
    }

    /// Regression: `sanitize_secret_value` tests emptiness BEFORE trimming and
    /// then stores the trimmed string, so a whitespace-only value cleared a bare
    /// `is_empty()` guard here and landed as `""` — the "configured but behaves
    /// like unset" state this guard exists to prevent.
    #[test]
    fn whitespace_is_not_a_value() {
        assert!(is_blank(""));
        assert!(is_blank(" "), "a space would otherwise be stored as empty");
        assert!(is_blank("\t\n  "));
        assert!(!is_blank("sk_test_123"));
        assert!(!is_blank("  padded  "), "a real value survives its padding");
    }

    /// The review's escape: `staging%2FKEY` decodes to `staging/KEY` in the
    /// path, which named staging's row on a production request.
    #[test]
    fn a_key_is_one_path_segment() {
        assert_eq!(bare_key(" QB_TOKEN ").unwrap(), "QB_TOKEN");
        for escape in ["staging/QB_TOKEN", "../KEY", "dev-x/K", "", "  "] {
            let (code, _) = bare_key(escape).unwrap_err();
            assert_eq!(code, StatusCode::BAD_REQUEST, "{escape:?}");
        }
    }
}
