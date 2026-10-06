//! Which environment's secrets a request to the secrets surface addresses.
//!
//! Production is the default and needs nothing beyond each mount's own guard.
//! **Every other environment is staff-only**: setting, reading or deleting
//! `apps/<id>/<env>/<KEY>` — staging's, or a sandbox's `dev-<handle>` — is
//! decided by oxy-authz (`Action::AppNonProduction`, via
//! `may_open_non_production` — the rule that serves a staging host and admits a
//! non-production `/fn`), so a tenant admin on the workspace mount, or an
//! operator without `develop_apps`, is refused. A sandbox must also exist: one
//! nobody created, or one being torn down, is a 404.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use super::Failure;

/// `?environment=staging` (or a sandbox's `dev-<handle>`) on the list, delete
/// and reveal routes.
#[derive(Debug, Default, Deserialize)]
pub struct EnvironmentQuery {
    #[serde(default)]
    pub environment: Option<String>,
}

/// The environment `raw` names: absent or blank is production; otherwise any
/// name `AppEnvironment::parse` accepts — `production`, `staging`, or a
/// sandbox's `dev-<handle>`. Anything else is a 400. Whether a named sandbox
/// exists is [`resolve`]'s question, asked after the caller is authorized.
pub(super) fn parse(raw: Option<&str>) -> Result<AppEnvironment, Failure> {
    let Some(name) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(AppEnvironment::Production);
    };
    AppEnvironment::parse(name).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "environment {name:?} holds no secrets: use \"production\" (the default), \
                 \"staging\", or a sandbox's \"dev-<handle>\""
            ),
        )
    })
}

/// Refuse a non-production environment to anyone oxy-authz does not let open
/// one for this app. Production passes: the mount's own guard decided it.
pub(super) async fn authorize(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<(), Failure> {
    if *environment == AppEnvironment::Production {
        return Ok(());
    }
    let caller = crate::server::authz::Caller::from_user(user);
    if crate::server::api::custom_apps_env_resolve::may_open_non_production(db, &caller, app).await
    {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        format!(
            "{environment} secrets are set by Oxy staff who may open this app's non-production \
             environments"
        ),
    ))
}

/// Outside production, which of `entries` a run reads from another
/// environment while this one stores no value of its own
/// ([`apply_inheritance`]): staging's value, in a sandbox; else production's,
/// for a key that is `shared` — marked so by both the declaring build and the
/// build production serves (`shared_env::effective_shared_env`).
pub(super) async fn mark_inherited(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    manifest: Option<&serde_json::Value>,
    entries: &mut [super::declared::AppSecretEntry],
) {
    if *environment == AppEnvironment::Production {
        return;
    }
    let shared = super::shared_env::effective_shared_env(db, app, environment, manifest).await;
    let staging_keys = staging_keys_for(db, app, environment).await;
    apply_inheritance(environment, &shared, &staging_keys, entries);
}

/// The keys staging holds, for a sandbox's view; empty for any other
/// environment, and — the view then under-reports inheritance rather than
/// failing — when staging's cannot be listed.
async fn staging_keys_for(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> BTreeSet<String> {
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return BTreeSet::new();
    }
    match super::stored_secrets(db, app, &AppEnvironment::Staging).await {
        Ok(stored) => stored.into_iter().map(|s| s.key).collect(),
        Err((_, e)) => {
            tracing::warn!(app_id = %app.id, "staging's secrets could not be listed: {e}");
            BTreeSet::new()
        }
    }
}

/// Mark what each unset entry would read instead, in the order
/// `scope::plan_env` resolves `ctx.env`: a sandbox reads staging's value
/// (`staging_keys`) before production's shared one; staging has no staging
/// layer above it.
fn apply_inheritance(
    environment: &AppEnvironment,
    shared: &BTreeSet<String>,
    staging_keys: &BTreeSet<String>,
    entries: &mut [super::declared::AppSecretEntry],
) {
    let reads_staging = matches!(environment, AppEnvironment::Dev { .. });
    for entry in entries.iter_mut() {
        entry.shared = shared.contains(&entry.key);
        entry.inherits_staging =
            reads_staging && !entry.is_set && staging_keys.contains(&entry.key);
        entry.inherits_production = entry.shared && !entry.is_set && !entry.inherits_staging;
    }
}

/// Who is asking: the authenticated user, and whether the credential was a
/// publish token — which is its minter, with a narrower grant.
#[derive(Clone, Copy)]
pub(super) struct Caller<'a> {
    pub user: &'a AuthenticatedUser,
    pub publish_token: bool,
}

impl<'a> Caller<'a> {
    pub(super) fn of(
        user: &'a AuthenticatedUser,
        marker: &Option<axum::Extension<oxy_auth::types::AppPublishTokenAuth>>,
    ) -> Self {
        Self {
            user,
            publish_token: marker.is_some(),
        }
    }
}

/// A publish token reaches production's secrets as it always has, and no
/// other environment's: every non-production operation refuses one, whoever
/// minted it. `403`, with this surface's plain-text body.
fn refuse_publish_token(caller: Caller<'_>, environment: &AppEnvironment) -> Result<(), Failure> {
    if !caller.publish_token || *environment == AppEnvironment::Production {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        format!(
            "publish_token_refused: a publish token cannot read or write {environment} \
             secrets; use a login token (`oxyc login`) or an API key"
        ),
    ))
}

/// [`parse`], the publish-token refusal, [`authorize`], then — for a sandbox
/// — that it exists: the environment a request may act on.
///
/// A sandbox nobody created, or one being torn down, is a 404: writing a
/// secret under its path would outlive it, or be handed to whoever creates
/// the name next. Asked last, so a caller who may not open non-production
/// learns nothing about which sandboxes exist.
pub(super) async fn resolve(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: Caller<'_>,
    raw: Option<&str>,
) -> Result<AppEnvironment, Failure> {
    let environment = parse(raw)?;
    refuse_publish_token(caller, &environment)?;
    authorize(db, app, caller.user, &environment).await?;
    ensure_exists(db, app, &environment).await?;
    Ok(environment)
}

/// A sandbox must have a row that is not being deleted; the fixed
/// environments always exist.
async fn ensure_exists(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<(), Failure> {
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Ok(());
    }
    let row = crate::server::api::custom_apps_env_resolve::sandbox_row(db, app.id, environment)
        .await
        .map_err(|e| {
            tracing::error!(app_id = %app.id, "sandbox lookup failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "environment lookup failed".to_string(),
            )
        })?;
    match row {
        Some(_) => Ok(()),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("this app has no environment {environment}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_is_the_default_and_any_environment_name_is_accepted() {
        assert_eq!(parse(None).unwrap(), AppEnvironment::Production);
        assert_eq!(parse(Some(" ")).unwrap(), AppEnvironment::Production);
        assert_eq!(
            parse(Some("production")).unwrap(),
            AppEnvironment::Production
        );
        assert_eq!(parse(Some("staging")).unwrap(), AppEnvironment::Staging);
        assert_eq!(
            parse(Some("dev-luong")).unwrap(),
            AppEnvironment::Dev {
                handle: "luong".into()
            }
        );
        assert_eq!(
            parse(Some(" dev-a1-b2 ")).unwrap(),
            AppEnvironment::Dev {
                handle: "a1-b2".into()
            }
        );
        for refused in ["Staging", "prod", "dev-", "dev--x", "dev-UPPER", "luong"] {
            let (code, message) = parse(Some(refused)).unwrap_err();
            assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
            assert!(message.contains("dev-<handle>"), "{message}");
        }
    }

    fn entry(key: &str, is_set: bool, required: bool) -> super::super::declared::AppSecretEntry {
        super::super::declared::AppSecretEntry {
            key: key.to_string(),
            is_set,
            declared: true,
            required,
            source: super::super::declared::EnvSource::Manifest,
            description: None,
            secret_id: None,
            updated_at: None,
            updated_by_email: None,
            shared: false,
            inherits_production: false,
            inherits_staging: false,
        }
    }

    /// In a sandbox view a key the sandbox holds no value of reads staging's
    /// when staging has one, and only otherwise production's shared value —
    /// the order `scope::plan_env` resolves `ctx.env` in.
    #[test]
    fn a_sandbox_entry_inherits_staging_before_shared_production() {
        let sandbox = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        let shared: BTreeSet<String> = ["SHARED".to_string(), "BOTH".to_string()].into();
        let staging: BTreeSet<String> =
            ["STG".to_string(), "BOTH".to_string(), "OWN".to_string()].into();
        let mut entries = vec![
            entry("OWN", true, true),
            entry("STG", false, true),
            entry("BOTH", false, true),
            entry("SHARED", false, true),
            entry("NONE", false, true),
        ];
        apply_inheritance(&sandbox, &shared, &staging, &mut entries);
        let seen: Vec<(&str, bool, bool, bool)> = entries
            .iter()
            .map(|e| {
                (
                    e.key.as_str(),
                    e.inherits_staging,
                    e.inherits_production,
                    e.is_missing_required(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("OWN", false, false, false),
                ("STG", true, false, false),
                ("BOTH", true, false, false),
                ("SHARED", false, true, false),
                ("NONE", false, false, true),
            ]
        );
    }

    /// Staging has no staging layer above it: it inherits production's
    /// shared keys only, as before sandboxes.
    #[test]
    fn staging_inherits_production_only() {
        let shared: BTreeSet<String> = ["SHARED".to_string()].into();
        let staging_keys: BTreeSet<String> = ["STG".to_string()].into();
        let mut entries = vec![entry("STG", false, true), entry("SHARED", false, true)];
        apply_inheritance(
            &AppEnvironment::Staging,
            &shared,
            &staging_keys,
            &mut entries,
        );
        assert!(!entries[0].inherits_staging);
        assert!(entries[0].is_missing_required());
        assert!(entries[1].inherits_production);
    }

    #[test]
    fn inherits_staging_is_serialized_only_when_true() {
        let mut e = entry("K", false, false);
        let json = serde_json::to_value(&e).unwrap();
        assert!(json.get("inherits_staging").is_none(), "{json}");
        e.inherits_staging = true;
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["inherits_staging"], true);
    }
}
