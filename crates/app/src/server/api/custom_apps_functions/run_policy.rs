//! **The one place a run's [`EnvPolicy`] is assembled.**
//!
//! `environment_gate::admit` decides *whether* a run happens and hands back
//! the environment's bare policy. What that policy still needs comes from the
//! database and from the build that will run: where `ctx.oltp` lands, the
//! semantic revision the build pins, the `env` keys it shares with
//! production, and where its warehouse writes are mapped. A route call and a
//! queued run used to gather those separately, and the queued path gathered
//! fewer — fine while a queued run was production-only, wrong the day a check
//! runs in an environment: a check must run under the policy a route call in
//! the same environment gets, or a pass predicts nothing.
//!
//! **Production asks nothing.** Every one of these is ignored by a production
//! policy (`with_semantic_pin`, `with_shared_env`, `with_destinations`,
//! `environment_gate::with_oltp_home`), so production returns its admission's
//! policy as it stands, with no query — the hottest path stays exactly as it
//! was, and a database blip there cannot fail a production run.
//!
//! **Outside production a lookup failure is an error**, never a fallback: the
//! caller refuses the run rather than start it under a thinner policy.

use entity::{app_builds, apps};
use sea_orm::{DatabaseConnection, DbErr};

use super::env_policy::EnvPolicy;
use super::environment_gate::{self, Admission};
use crate::server::api::custom_apps_nonproduction::destinations_from_build_manifest;
use crate::server::api::custom_apps_secrets::shared_env::effective_shared_env;

/// The policy `admission`'s run of `build` executes under.
///
/// Production: the admission's own policy. Otherwise, in order: the org's
/// OLTP staging branch, the build's semantic pin, the `env` keys both this
/// build and production's mark `shared`, and the build's
/// `nonProduction.destinations`.
pub(crate) async fn build(
    db: &DatabaseConnection,
    admission: Admission,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<EnvPolicy, DbErr> {
    if admission.policy.is_production() {
        return Ok(admission.policy);
    }
    let admission = environment_gate::with_oltp_home(db, admission, app).await?;
    let manifest = build.manifest_json.as_ref();
    let shared = effective_shared_env(db, app, admission.policy.environment(), manifest).await;
    Ok(with_build_pin(db, admission.policy, build)
        .await
        .with_shared_env(shared)
        // Where a non-production write to a customer warehouse lands, from the
        // manifest that shipped with this build.
        .with_destinations(destinations_from_build_manifest(manifest, app.id)))
}

/// `policy`, reading the semantic model `build` pins. Only a node built with
/// the isolate reads the pin; one built without it never runs the function.
#[cfg(feature = "custom-app-functions")]
async fn with_build_pin(
    db: &DatabaseConnection,
    policy: EnvPolicy,
    build: &app_builds::Model,
) -> EnvPolicy {
    environment_gate::with_build_pin(db, policy, build.id).await
}

#[cfg(not(feature = "custom-app-functions"))]
async fn with_build_pin(
    _db: &DatabaseConnection,
    policy: EnvPolicy,
    _build: &app_builds::Model,
) -> EnvPolicy {
    policy
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use oxy_app_core::custom_app_environment::AppEnvironment;
    use uuid::Uuid;

    use super::*;
    use crate::server::api::custom_apps_env_resolve::ResolvedEnvironment;
    use crate::server::api::custom_apps_functions::environment_gate::Entrance;

    fn app_row() -> apps::Model {
        let now = chrono::Utc::now().fixed_offset();
        apps::Model {
            visibility: "org".to_string(),
            id: Uuid::from_u128(9),
            slug: "x".to_string(),
            name: "X".to_string(),
            org_id: Uuid::from_u128(7),
            project_id: Uuid::nil(),
            branch: "main".to_string(),
            source_repo: String::new(),
            status: "created".to_string(),
            source_type: "s3".to_string(),
            source_config: serde_json::json!({}),
            bootstrap_pr_url: None,
            last_synced_at: None,
            manifest_override: None,
            published_at: None,
            repo_path: None,
            draft_build_id: Some(Uuid::from_u128(2)),
            published_build_id: Some(Uuid::from_u128(1)),
            last_promoted_by: None,
            last_promoted_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// A build whose manifest declares everything a non-production policy
    /// reads: a shared `env` key and a warehouse mapping.
    fn build_row() -> app_builds::Model {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(2),
            "app_id": Uuid::from_u128(9),
            "build_id": "b-2",
            "s3_prefix": "customer-apps/x/builds/b-2/",
            "manifest_json": {
                "env": { "API_URL": { "shared": true } },
                "nonProduction": { "destinations": { "clickhouse": "clickhouse_staging" } },
            },
            "created_at": chrono::Utc::now().fixed_offset(),
            "published_by": null,
            "published_via": null,
            "source_repo": null,
            "commit_sha": null,
            "source_branch": null,
            "validation_status": "passed",
            "validation_detail": null,
            "semantic_revision_id": Uuid::from_u128(5),
        }))
        .expect("an app_builds row")
    }

    fn admitted(environment: AppEnvironment, entrance: Entrance) -> Admission {
        environment_gate::admit(
            &ResolvedEnvironment {
                environment,
                build_id: Some(Uuid::from_u128(2)),
            },
            entrance,
        )
        .expect("admitted")
    }

    /// Production is the admission's policy untouched, with no query: the
    /// connection is disconnected, so any statement panics. The build's pin,
    /// shared keys and mapping are all there to be (wrongly) picked up.
    #[tokio::test]
    async fn run_policy_for_production_is_the_admission_policy_and_asks_nothing() {
        let db = DatabaseConnection::default();
        for entrance in [
            Entrance::Queued,
            Entrance::Route {
                non_production_reach: false,
            },
        ] {
            let policy = build(
                &db,
                admitted(AppEnvironment::Production, entrance),
                &app_row(),
                &build_row(),
            )
            .await
            .expect("production needs no lookup");
            assert_eq!(policy, EnvPolicy::production(), "{entrance:?}");
            assert_eq!(policy.semantic_pin(), None, "production never reads a pin");
            assert_eq!(policy.mapped_destination("clickhouse"), None);
        }
    }

    /// Outside production the policy is looked up, and a run never proceeds
    /// on a policy it could not finish: with no database the answer is an
    /// error (or the disconnected fake's panic) — not a production policy,
    /// and not the bare one either.
    #[tokio::test]
    async fn run_policy_outside_production_fails_rather_than_run_on_a_thinner_policy() {
        let db = DatabaseConnection::default();
        let staging = admitted(
            AppEnvironment::Staging,
            Entrance::Route {
                non_production_reach: true,
            },
        );
        let outcome = std::panic::AssertUnwindSafe(build(&db, staging, &app_row(), &build_row()))
            .catch_unwind()
            .await;
        assert!(
            !matches!(outcome, Ok(Ok(_))),
            "a staging policy needs the OLTP branch lookup; without it the run must not start"
        );
    }
}
