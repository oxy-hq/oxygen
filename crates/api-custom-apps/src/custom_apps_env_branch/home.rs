//! Where a sandbox's `ctx.oltp` lands when its planes ask for a branch of its
//! own: decided once, at admission (`environment_gate::with_oltp_home`).
//!
//! One answer per invocation, read from the same resolver the call connects
//! through (`oxy_oltp::resolver::resolve_env_branch_writer_for_org`, accepting
//! only `ready`), so what admission calls ready is what a connection would
//! serve. A branch that is not there, not ready, or cut in the other mode
//! than the planes now ask for, is a refusal that names its fix
//! ([`EnvBranchUnready`]) — **never** staging's schema, a shared schema or
//! production.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::OltpMode;
use oxy_oltp::WriterRef;
use oxy_oltp::entity::env_branches::EnvBranchMode;
use oxy_oltp::env_branches::find_for_app;
use oxy_oltp::resolver::{
    EnvBranchAccept, ResolveError, WriterConnection, resolve_env_branch_writer_for_org,
};
use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use crate::custom_apps_functions::env_policy::{
    EnvBranchHome, EnvBranchUnready, EnvPolicy, OltpHome,
};

/// The mode of a sandbox whose planes ask for an OLTP branch of its own
/// (`branch` or `empty`). Staging never does, and neither does a sandbox on
/// `schema`: both keep the org's staging branch.
pub(crate) fn own_branch_mode(policy: &EnvPolicy) -> Option<OltpMode> {
    let mode = policy.planes()?.oltp;
    let sandbox = matches!(policy.environment(), AppEnvironment::Dev { .. });
    (sandbox && mode != OltpMode::Schema).then_some(mode)
}

/// The branch mode `mode` asks for; `None` for `schema`, which asks for none.
fn branch_mode(mode: OltpMode) -> Option<EnvBranchMode> {
    match mode {
        OltpMode::Branch => Some(EnvBranchMode::Branch),
        OltpMode::Empty => Some(EnvBranchMode::Empty),
        OltpMode::Schema => None,
    }
}

/// The home of `environment`'s `ctx.oltp`, when its planes' `mode` asks for a
/// branch (`branch` or `empty`; [`own_branch_mode`]). A branch ready in the
/// *other* mode is refused ([`EnvBranchUnready::CutAsBranch`] /
/// [`EnvBranchUnready::CutAsEmpty`]): `empty` exists so the sandbox does not
/// see production's rows.
///
/// `Err` is a lookup that failed, a `schema` mode (not a branch's), or a row
/// that names production's own database: the caller refuses the run rather
/// than start it on a home it could not finish deciding.
pub async fn home_for(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    mode: OltpMode,
) -> Result<OltpHome, DbErr> {
    Ok(match branch_for(db, app, environment, mode).await? {
        Ok(resolved) => OltpHome::EnvironmentBranch(resolved.home),
        Err(why) => unready(why),
    })
}

/// A branch the planes' mode asks for, resolved once: the home a call would be
/// admitted to, and the connection that reaches it. `env diff` reads through
/// the connection it was admitted on, so what it compares is what `ctx.oltp`
/// would serve.
#[derive(Debug, Clone)]
pub struct ResolvedBranch {
    pub home: EnvBranchHome,
    pub connection: WriterConnection,
}

/// [`home_for`], keeping the connection: `Ok(Ok(branch))` when `environment`'s
/// branch is ready and cut in the mode `mode` asks for, `Ok(Err(why))` when it
/// is not (the refusal that names its fix), `Err` as for [`home_for`].
pub async fn branch_for(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    mode: OltpMode,
) -> Result<Result<ResolvedBranch, EnvBranchUnready>, DbErr> {
    let wanted = branch_mode(mode)
        .ok_or_else(|| DbErr::Custom(format!("the {mode:?} mode asks for no OLTP branch")))?;
    let Some(writer) =
        oxy_oltp::schema::app_writer_name(&app.slug).and_then(|name| WriterRef::app(name).ok())
    else {
        return Ok(Err(EnvBranchUnready::NoTenant));
    };
    let env_name = environment.name();
    let resolved = resolve_env_branch_writer_for_org(
        db,
        app.org_id,
        app.id,
        &env_name,
        &writer,
        EnvBranchAccept::Ready,
    )
    .await;
    match resolved {
        Ok(Some(branch)) => {
            let row = find_for_app(db, app.id, &env_name).await?;
            let Some(cut_as) = row.map(|row| row.mode) else {
                return Ok(Err(EnvBranchUnready::NotCut));
            };
            if cut_as != wanted {
                return Ok(Err(cut_as_other(cut_as)));
            }
            Ok(Ok(ResolvedBranch {
                home: EnvBranchHome {
                    cut: branch.cut,
                    app_id: app.id,
                    env_name,
                },
                connection: branch.connection,
            }))
        }
        Ok(None) => Ok(Err(EnvBranchUnready::NotCut)),
        Err(e) => unready_of_error(e, app.id, &env_name).map(Err),
    }
}

fn unready(why: EnvBranchUnready) -> OltpHome {
    OltpHome::EnvironmentBranchUnready(why)
}

/// The refusal for a branch cut as `cut_as` when the planes ask for the other.
fn cut_as_other(cut_as: EnvBranchMode) -> EnvBranchUnready {
    match cut_as {
        EnvBranchMode::Branch => EnvBranchUnready::CutAsBranch,
        EnvBranchMode::Empty => EnvBranchUnready::CutAsEmpty,
    }
}

/// What a resolver refusal means for the home. Every arm is a refusal or an
/// error; none is a home.
#[cfg(test)]
fn from_error(error: ResolveError, app_id: Uuid, env_name: &str) -> Result<OltpHome, DbErr> {
    unready_of_error(error, app_id, env_name).map(unready)
}

/// The refusal a resolver error is, or the error when it is not one.
fn unready_of_error(
    error: ResolveError,
    app_id: Uuid,
    env_name: &str,
) -> Result<EnvBranchUnready, DbErr> {
    match error {
        ResolveError::EnvBranchNotReady { status, .. } => Ok(not_ready(status)),
        ResolveError::NotProvisioned(_)
        | ResolveError::NotActive(..)
        | ResolveError::WriterNotProvisioned { .. } => Ok(EnvBranchUnready::NoTenant),
        // A credential the cut never minted: the next reset mints it.
        ResolveError::EnvBranchCredentialMissing { .. } => Ok(EnvBranchUnready::Failed),
        ResolveError::EnvBranchIsProduction { .. } => {
            tracing::error!(%app_id, environment = %env_name,
                "the environment's OLTP branch row names production's own database; \
                 refusing to admit a run to it");
            Err(DbErr::Custom(
                "the environment's OLTP branch is recorded as production's own database".into(),
            ))
        }
        ResolveError::Db(e) => Err(e),
        other => Err(DbErr::Custom(other.to_string())),
    }
}

/// The unready reason for a branch row in `status`: a cut, migration or reset
/// in flight is `Cutting`; only `failed` is `Failed`. `status` is the
/// resolver's `&'static str` of `EnvBranchStatus::as_str`, so a renamed
/// status shows in the test below, not here.
fn not_ready(status: &str) -> EnvBranchUnready {
    match status {
        "failed" => EnvBranchUnready::Failed,
        _ => EnvBranchUnready::Cutting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_oltp::entity::env_branches::EnvBranchStatus;

    fn refused(error: ResolveError) -> Result<OltpHome, DbErr> {
        from_error(error, Uuid::nil(), "dev-a1")
    }

    fn unready_of(error: ResolveError) -> EnvBranchUnready {
        match refused(error).expect("a refusal, not an error") {
            OltpHome::EnvironmentBranchUnready(why) => why,
            other => panic!("a refusal must never be a home: {other:?}"),
        }
    }

    /// The plane decides for a sandbox alone: `branch` and `empty` of a
    /// sandbox ask for a branch; `schema` does not; staging and production
    /// never do, whatever the planes they are handed say.
    #[test]
    fn only_a_sandbox_on_branch_or_empty_asks_for_its_own_branch() {
        use crate::custom_apps_functions::env_policy::EnvPolicy;
        use crate::custom_apps_planes::PlaneSpec;
        let policy = |environment: AppEnvironment, oltp: &str| {
            let planes = PlaneSpec::parse(&serde_json::json!({ "oltp": oltp }))
                .expect("a mode")
                .resolve(&environment)
                .expect("planes");
            EnvPolicy::for_environment(environment).with_planes(planes)
        };
        let sandbox = || AppEnvironment::parse("dev-a1").expect("a sandbox");
        for (oltp, asks) in [
            ("branch", Some(OltpMode::Branch)),
            ("empty", Some(OltpMode::Empty)),
            ("schema", None),
        ] {
            assert_eq!(own_branch_mode(&policy(sandbox(), oltp)), asks, "{oltp}");
            assert_eq!(
                own_branch_mode(&policy(AppEnvironment::Staging, oltp)),
                None,
                "staging {oltp}"
            );
        }
        let production = EnvPolicy::production();
        assert_eq!(
            own_branch_mode(&production),
            None,
            "production has no planes"
        );
    }

    #[test]
    fn the_resolvers_status_strings_map_to_cutting_and_failed() {
        use EnvBranchStatus::*;
        for status in [Cutting, Migrating, Resetting] {
            assert_eq!(not_ready(status.as_str()), EnvBranchUnready::Cutting);
        }
        assert_eq!(not_ready(Failed.as_str()), EnvBranchUnready::Failed);
    }

    /// The last door to production: a row that names its database is an error
    /// that refuses the run, never a home and never a quiet refusal.
    #[test]
    fn a_row_naming_production_is_an_error_never_a_home() {
        let error = refused(ResolveError::EnvBranchIsProduction {
            app_id: Uuid::nil(),
            env_name: "dev-a1".into(),
        })
        .expect_err("refused as an error");
        assert!(
            error.to_string().contains("production's own database"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_credential_is_failed_and_no_tenant_is_no_tenant() {
        let missing = ResolveError::EnvBranchCredentialMissing {
            app_id: Uuid::nil(),
            env_name: "dev-a1".into(),
            role: "r".into(),
        };
        assert_eq!(unready_of(missing), EnvBranchUnready::Failed);
        for error in [
            ResolveError::NotProvisioned(Uuid::nil()),
            ResolveError::NotActive(Uuid::nil(), "suspended"),
            ResolveError::WriterNotProvisioned {
                org_id: Uuid::nil(),
                writer: "app:x".into(),
            },
        ] {
            assert_eq!(unready_of(error), EnvBranchUnready::NoTenant);
        }
    }

    #[test]
    fn a_lookup_failure_refuses_the_run() {
        assert!(refused(ResolveError::Db(DbErr::Custom("down".into()))).is_err());
        assert!(refused(ResolveError::Crypto("bad".into())).is_err());
    }

    #[test]
    fn a_branch_cut_in_the_other_mode_names_the_mode_it_was_cut_in() {
        assert_eq!(
            cut_as_other(EnvBranchMode::Branch),
            EnvBranchUnready::CutAsBranch
        );
        assert_eq!(
            cut_as_other(EnvBranchMode::Empty),
            EnvBranchUnready::CutAsEmpty
        );
        assert_eq!(branch_mode(OltpMode::Schema), None);
        assert_eq!(branch_mode(OltpMode::Empty), Some(EnvBranchMode::Empty));
    }
}
