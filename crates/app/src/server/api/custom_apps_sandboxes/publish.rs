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
use crate::server::api::custom_apps_env_resolve::may_open_environment;
use crate::server::api::custom_apps_environments::{EnvAction, record_move};
use crate::server::api::custom_apps_migrations::DeclaredMigration;
use crate::server::api::custom_apps_nonproduction::staging_task::QueuedMigration;
use crate::server::api::custom_apps_publish::{
    PublishError, PublishInput, PublishTarget, ensure_same_workspace,
};

/// The credential a publish request arrived with, as far as its target cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishCredential {
    /// A session, a legacy key, or a personal or service-account token.
    Other,
    /// An `oxypublish_` token: the channels only, never a sandbox.
    PublishToken,
    /// A sandbox agent token (`oxy_sbx_`): a sandbox only, never the channels.
    SandboxAgent,
}

impl PublishCredential {
    /// The credential of a request: the publish-token marker, or the kind of
    /// token the user authenticated with.
    pub fn of(
        user: &oxy_auth::types::AuthenticatedUser,
        marker: Option<&oxy_auth::types::AppPublishTokenAuth>,
    ) -> Self {
        let agent = user
            .credential
            .as_ref()
            .is_some_and(|credential| credential.is_sandbox_agent());
        match (marker, agent) {
            (Some(_), _) => Self::PublishToken,
            (None, true) => Self::SandboxAgent,
            (None, false) => Self::Other,
        }
    }
}

/// The target a publish request names. `environment` absent or empty is
/// today's publish. Otherwise it must be a sandbox's name
/// (`InvalidEnvironment`), must not arrive with `promote` or
/// `channel=published` (`SandboxWithPromote`), and must not be authenticated
/// by a publish token (`SandboxRefused`).
///
/// A sandbox agent token is the other way round: it names a sandbox, and a
/// publish with no `environment`, or with `promote`, is `SandboxTokenRefused`.
pub fn target_of(
    environment: Option<&str>,
    promote: bool,
    credential: PublishCredential,
) -> Result<PublishTarget, PublishError> {
    let agent = credential == PublishCredential::SandboxAgent;
    let Some(name) = environment.map(str::trim).filter(|name| !name.is_empty()) else {
        if agent {
            return Err(PublishError::SandboxTokenRefused);
        }
        return Ok(PublishTarget::Channels);
    };
    let Some(sandbox @ AppEnvironment::Dev { .. }) = AppEnvironment::parse(name) else {
        return Err(PublishError::InvalidEnvironment(name.to_string()));
    };
    if promote {
        if agent {
            return Err(PublishError::SandboxTokenRefused);
        }
        return Err(PublishError::SandboxWithPromote);
    }
    if credential == PublishCredential::PublishToken {
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
    // Asked of the one sandbox: a sandbox agent token opens only a sandbox it
    // created, of an app it is granted. For everyone else this is the app's
    // non-production rule, as before.
    if !may_open_environment(db, caller, app, environment).await {
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

/// Who moves a sandbox's pointer: the user the event names, and the sandbox
/// agent token the publish arrived with, when that is its credential.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mover {
    pub actor: Option<Uuid>,
    /// `Some` holds the move to a sandbox that token created, decided on the
    /// row the move locks.
    pub own: Option<Uuid>,
}

/// Point the sandbox at `build_pk`, with its event, and move nothing else. A
/// sandbox deleted since [`admit`] has no row to move: `EnvironmentDeleting`,
/// and the caller rolls the stored build back. So does one a sandbox agent
/// token did not create, when the token is the mover.
pub(crate) async fn move_pointer(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
    build_pk: Uuid,
    mover: Mover,
) -> Result<(), PublishError> {
    let db_err = |e: DbErr| PublishError::Db(e.to_string());
    let txn = db.begin().await.map_err(db_err)?;
    let moved = async {
        if let Some(token) = mover.own
            && !super::own::lock_own(&txn, app_id, environment, token).await?
        {
            return Err(DbErr::RecordNotFound(format!(
                "app {app_id} has no sandbox {environment} this token created"
            )));
        }
        let action = EnvAction::Publish;
        record_move(
            &txn,
            app_id,
            environment,
            Some(build_pk),
            action,
            mover.actor,
        )
        .await
    }
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
#[path = "publish_tests.rs"]
mod tests;
