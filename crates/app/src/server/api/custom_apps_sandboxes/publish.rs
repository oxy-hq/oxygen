//! What a publish that names a sandbox checks, moves and queues
//! (`internal-docs/custom-app-sandboxes.md` §5.2). `custom_apps_publish`
//! stores the build exactly as it stores any other; these are the places a
//! sandbox publish differs, kept out of that file:
//!
//! * [`target_of`] — the request's `environment` field to a [`PublishTarget`];
//! * [`admit_target`] — who may publish to a sandbox, and that the sandbox exists;
//! * [`move_pointer`] — the sandbox's pointer, and nothing else;
//! * [`queue_migrations`] — the sandbox's Airhouse sibling and its own OLTP
//!   schema on the org's staging branch, each queued.
//!
//! **A sandbox publish never touches the app.** It does not create or rename
//! the app row, move `apps.draft_build_id`, mirror staging, register a
//! schedule, or apply a migration to production or to staging's schema on the
//! org's OLTP staging branch. Two sandboxes of one app must not fight over
//! production's label.

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait, TransactionTrait};
use uuid::Uuid;

use super::migrations_task::{self, SandboxMigrationsTask};
use super::oltp_task::{self, SandboxOltpTask};
use crate::server::api::custom_apps_env_resolve::may_open_non_production;
use crate::server::api::custom_apps_environments::{EnvAction, record_move};
use crate::server::api::custom_apps_migrations::DeclaredMigration;
use crate::server::api::custom_apps_nonproduction::staging_task::QueuedMigration;
use crate::server::api::custom_apps_publish::{
    PublishError, PublishInput, PublishTarget, ensure_same_workspace,
};

/// The target a publish request names. `environment` absent or empty is
/// today's publish. Otherwise it must be a sandbox's name
/// (`InvalidEnvironment`), must not arrive with `promote` or
/// `channel=published` (`SandboxWithPromote`), and must not be authenticated
/// by a publish token (`SandboxRefused`).
pub fn target_of(
    environment: Option<&str>,
    promote: bool,
    publish_token: bool,
) -> Result<PublishTarget, PublishError> {
    let Some(name) = environment.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(PublishTarget::Channels);
    };
    let Some(sandbox @ AppEnvironment::Dev { .. }) = AppEnvironment::parse(name) else {
        return Err(PublishError::InvalidEnvironment(name.to_string()));
    };
    if promote {
        return Err(PublishError::SandboxWithPromote);
    }
    if publish_token {
        return Err(PublishError::SandboxRefused);
    }
    Ok(PublishTarget::Sandbox(sandbox))
}

/// [`admit`] for whatever a publish targets, decided before the bundle is
/// inflated: `None` for the channels' publish, which this module has no say
/// over; `Some(the app's id)` for a sandbox that admits it.
pub(crate) async fn admit_target(
    db: &DatabaseConnection,
    input: &PublishInput,
    app: Option<&apps::Model>,
    target: &PublishTarget,
) -> Result<Option<Uuid>, PublishError> {
    match target {
        PublishTarget::Channels => Ok(None),
        PublishTarget::Sandbox(environment) => admit(db, input, app, environment).await.map(Some),
    }
}

/// Whether this publish may go to `environment`, checked after
/// `authorize_publish` and before the bundle is inflated. Answers the app's id.
///
/// In order: a machine token is no staff identity; the app must exist; the publisher must be allowed to open its
/// non-production environments (the rule that opens staging); the publish
/// must name the app's own workspace — a sandbox publish never re-homes an
/// app; and the sandbox must exist and not be on its way out.
async fn admit(
    db: &DatabaseConnection,
    input: &PublishInput,
    app: Option<&apps::Model>,
    environment: &AppEnvironment,
) -> Result<Uuid, PublishError> {
    // The publisher with the credential the request arrived with: a token that
    // carries no staff standing opens no sandbox.
    let (Some(_), Some(_), Some(caller), None, None) = (
        input.published_by,
        input.published_by_email.as_deref(),
        input.publisher.as_ref(),
        input.machine_app_id,
        input.published_via.as_ref(),
    ) else {
        return Err(PublishError::SandboxRefused);
    };
    let name = environment.name();
    let Some(app) = app else {
        return Err(PublishError::UnknownEnvironment { name });
    };
    if !may_open_non_production(db, caller, app).await {
        return Err(PublishError::SandboxRefused);
    }
    if ensure_same_workspace(db, app, input).await? {
        return Err(PublishError::ProjectMismatch {
            app_slug: app.slug.clone(),
            app_id: app.id,
            existing_project: app.project_id,
            requested_project: input.project_id,
        });
    }
    let row = app_environments::Entity::find_by_id((app.id, name.clone()))
        .one(db)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    match row {
        Some(row) if row.deleting_at.is_none() => Ok(app.id),
        Some(_) => Err(PublishError::EnvironmentDeleting { name }),
        None => Err(PublishError::UnknownEnvironment { name }),
    }
}

/// Point the sandbox at `build_pk`, with its event, and move nothing else. A
/// sandbox deleted since [`admit`] has no row to move: `EnvironmentDeleting`,
/// and the caller rolls the stored build back.
pub(crate) async fn move_pointer(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_pk: Uuid,
    actor: Option<Uuid>,
) -> Result<(), PublishError> {
    let db_err = |e: DbErr| PublishError::Db(e.to_string());
    let txn = db.begin().await.map_err(db_err)?;
    let moved = record_move(
        &txn,
        app_id,
        environment,
        Some(build_pk),
        EnvAction::Publish,
        actor,
    )
    .await;
    match moved {
        Ok(()) => txn.commit().await.map_err(db_err),
        Err(DbErr::RecordNotFound(_)) => Err(PublishError::EnvironmentDeleting {
            name: environment.name(),
        }),
        Err(e) => Err(db_err(e)),
    }
}

/// The build a publish queues a sandbox's migrations for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SandboxBuild<'a> {
    pub app_id: Uuid,
    pub app_slug: &'a str,
    /// The app's workspace: where the run is filed, and whose Airhouse the
    /// sibling schema lives in.
    pub workspace_id: Uuid,
    /// Whose OLTP staging branch the sandbox's own schema lives in.
    pub org_id: Uuid,
    pub build_pk: Uuid,
    pub environment: &'a AppEnvironment,
}

/// The last thing a sandbox publish does: queue the sandbox's own OLTP schema
/// on the org's staging branch — seeded if need be, then migrated with the
/// build's OLTP files — and the build's Airhouse migrations for the sandbox's
/// sibling. The warnings are the publish's.
pub(crate) async fn queue_migrations(
    db: &DatabaseConnection,
    build: SandboxBuild<'_>,
    oltp: &[DeclaredMigration],
    airhouse: &[DeclaredMigration],
) -> Vec<String> {
    let mut warnings = queue_oltp_schema(db, build, oltp).await;
    if airhouse.is_empty() {
        return warnings;
    }
    let task = SandboxMigrationsTask {
        app_id: build.app_id,
        app_slug: build.app_slug.to_string(),
        workspace_id: build.workspace_id,
        build_pk: build.build_pk,
        environment: build.environment.name(),
        migrations: queued(airhouse),
    };
    if let Err(e) = migrations_task::enqueue(db, &task).await {
        tracing::warn!(app_id = %build.app_id, environment = %build.environment, error = %e,
            "publish: could not queue the sandbox's Airhouse migrations");
        warnings.push(format!(
            "{}'s Airhouse migrations were not queued ({e}); the publish went on, and the \
             sandbox's Airhouse writes will fail until a later publish to it migrates it",
            build.environment
        ));
    }
    warnings
}

fn queued(declared: &[DeclaredMigration]) -> Vec<QueuedMigration> {
    declared
        .iter()
        .map(|m| QueuedMigration {
            filename: m.filename.clone(),
            checksum: m.checksum.clone(),
            sql: m.sql.clone(),
        })
        .collect()
}

/// Queue the sandbox's OLTP schema task when the org has an active staging
/// branch and the app an OLTP writer (`oltp_home::wants_schema`) — for every
/// such publish, whether or not the bundle declares OLTP migrations: the
/// schema must be there for the build's `ctx.oltp`. With no branch nothing is
/// queued, and files the bundle declares are named in a warning rather than
/// dropped in silence.
async fn queue_oltp_schema(
    db: &DatabaseConnection,
    build: SandboxBuild<'_>,
    oltp: &[DeclaredMigration],
) -> Vec<String> {
    let environment = build.environment;
    let not_queued = |why: String| {
        tracing::warn!(app_id = %build.app_id, %environment, "publish: {why}");
        vec![format!(
            "{environment}'s own OLTP schema was not queued ({why}); the publish went on, and \
             the sandbox's ctx.oltp may be refused until a later publish to it queues it"
        )]
    };
    match super::oltp_home::wants_schema(db, build.org_id, build.app_slug).await {
        Ok(true) => {}
        Ok(false) => return oltp_warning(environment, oltp.len()).into_iter().collect(),
        Err(e) => return not_queued(e),
    }
    let task = SandboxOltpTask {
        app_id: build.app_id,
        app_slug: build.app_slug.to_string(),
        org_id: build.org_id,
        workspace_id: build.workspace_id,
        build_pk: build.build_pk,
        environment: environment.name(),
        migrations: queued(oltp),
    };
    match oltp_task::enqueue(db, &task).await {
        Ok(_) => Vec::new(),
        Err(e) => not_queued(e.to_string()),
    }
}

/// With no OLTP staging branch (or no OLTP writer) a sandbox has no schema of
/// its own to migrate; `declared` files are named in a warning rather than
/// dropped in silence.
fn oltp_warning(environment: &AppEnvironment, declared: usize) -> Option<String> {
    (declared > 0).then(|| {
        format!(
            "{declared} OLTP migration file(s) were not applied: a publish to {environment} \
             applies them to the sandbox's own schema on the org's OLTP staging branch, and \
             this org has no active branch (or the app no OLTP writer). The sandbox's ctx.oltp \
             writes are held; provision one with `oxyc oltp provision --branch staging`"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(handle: &str) -> AppEnvironment {
        AppEnvironment::Dev {
            handle: handle.into(),
        }
    }

    /// Absent or empty is today's publish, whatever else was sent.
    #[test]
    fn no_environment_is_the_channels_publish() {
        for absent in [None, Some(""), Some("   ")] {
            for (promote, token) in [(false, false), (true, false), (false, true), (true, true)] {
                assert!(matches!(
                    target_of(absent, promote, token),
                    Ok(PublishTarget::Channels)
                ));
            }
        }
    }

    #[test]
    fn a_sandbox_name_selects_that_sandbox() {
        let target = target_of(Some(" dev-a1 "), false, false).expect("a sandbox");
        assert!(matches!(target, PublishTarget::Sandbox(env) if env == sandbox("a1")));
    }

    /// Only a sandbox can be named: staging and production are reached by the
    /// publish this field leaves alone.
    #[test]
    fn a_name_that_is_not_a_sandboxs_is_invalid() {
        for name in ["staging", "production", "dev-", "dev--x", "a1", "DEV-a1"] {
            let refused = target_of(Some(name), false, false).expect_err(name);
            assert!(
                matches!(&refused, PublishError::InvalidEnvironment(n) if n == name),
                "{name}: {refused}"
            );
        }
    }

    /// The refusals in order: the name, then `promote`, then the credential.
    #[test]
    fn promote_and_a_publish_token_are_refused_with_a_sandbox() {
        assert!(matches!(
            target_of(Some("dev-a1"), true, false),
            Err(PublishError::SandboxWithPromote)
        ));
        assert!(matches!(
            target_of(Some("dev-a1"), false, true),
            Err(PublishError::SandboxRefused)
        ));
        assert!(matches!(
            target_of(Some("dev-a1"), true, true),
            Err(PublishError::SandboxWithPromote)
        ));
        assert!(matches!(
            target_of(Some("nope"), true, true),
            Err(PublishError::InvalidEnvironment(_))
        ));
    }

    /// With no branch, declared files are named — and so is how to get one.
    #[test]
    fn with_no_branch_declared_oltp_migrations_are_named_in_a_warning() {
        assert_eq!(oltp_warning(&sandbox("a1"), 0), None);
        let warning = oltp_warning(&sandbox("a1"), 2).expect("a warning");
        assert!(
            warning.starts_with("2 OLTP migration file(s) were not applied"),
            "{warning}"
        );
        assert!(warning.contains("dev-a1"), "{warning}");
        assert!(
            warning.contains("oxyc oltp provision --branch staging"),
            "{warning}"
        );
    }
}
