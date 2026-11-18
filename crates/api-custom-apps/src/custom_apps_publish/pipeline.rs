//! The publish as a pipeline of stages: admit → inflate → store → record →
//! gate → promote → point → after. A child of `custom_apps_publish`, which owns the
//! types and the helpers each stage calls; this file owns the order, and
//! [`OrphanGuard`] the one compensation that undoes a publish that failed
//! after it started writing.
//!
//! `admit` and `inflate` write nothing. The app row is then upserted and the
//! guard armed; `store`, `record`, `gate`, `promote` and `point` run under it,
//! so a failure in any of them undoes whatever was written so far, once — a
//! promoting publish production refuses (`production_gate`, then the event
//! gate) included, staging half and all. `after` cannot fail the publish.
//!
//! Each stage is clocked under its [`Stage`] name, and every publish whose
//! caller is authorized for the app — landed or refused — leaves one attempt
//! row with the timings and, for a refusal, the stage and the error
//! (`custom_apps_publish_attempts`). A caller refused authorization leaves
//! none: nothing it sends is written into an app it may not publish. The app
//! row's upsert is the first write, and is clocked as part of `store`.

use axum::body::Bytes;
use entity::organizations;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::admit::{admit, authorize};
use super::inflate::{Inflated, inflate};
use super::orphan::OrphanGuard;
use super::production_gate;
use super::{
    PROMOTING_PUBLISH_DEPRECATED, PublishError, PublishInput, PublishOptions, PublishResult,
    PublishTarget, app_row_for, asset_manifest, build_id_taken, cache, drop_resolution_cache,
    env_schedules, gc_builds, migrations, move_pointers, queue_migrations_for,
    reconcile_moved_environment, record_build, record_functions, sandbox_agent_publish,
    staging_task, store,
};
use crate::app_events::subscriptions;
use crate::custom_apps_publish_attempts::{Attempt, AttemptFacts, Failed, Stage, StageTimer};
use oxy_app_core::custom_app_environment::AppEnvironment;

/// [`super::publish_to`], once the credential has been held to its target:
/// the stages, timed, and the attempt they make recorded.
pub(super) async fn store_and_point(
    input: PublishInput,
    target: PublishTarget,
    options: PublishOptions,
) -> Result<PublishResult, Failed<PublishError>> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| Failed::unstaged(PublishError::Db(e.to_string())))?;
    let mut attempt = Attempt::begin(attempt_facts(&input, &target));
    let published = stages(&db, input, &target, options, &mut attempt).await;
    attempt
        .finish(&db, published.as_ref().err().map(ToString::to_string))
        .await;
    match published {
        Ok(result) => Ok(PublishResult {
            stages: attempt.timings(),
            ..result
        }),
        Err(error) => Err(Failed {
            error,
            stage: attempt.failed_stage(),
        }),
    }
}

/// What the attempt row says of the publish before it runs.
fn attempt_facts(input: &PublishInput, target: &PublishTarget) -> AttemptFacts {
    AttemptFacts {
        app_slug: input.app_slug.clone(),
        build_id: input.build_id.clone(),
        environment: target.moved(input.promote).0.name(),
        published_by: input.published_by,
        published_via: input.published_via.clone(),
        published_token_id: sandbox_agent_publish::author(input),
    }
}

/// The stages in order, each clocked on `attempt`, which learns the app and
/// the build as they are settled.
async fn stages(
    db: &DatabaseConnection,
    mut input: PublishInput,
    target: &PublishTarget,
    options: PublishOptions,
    attempt: &mut Attempt,
) -> Result<PublishResult, PublishError> {
    // Admit, in two pieces clocked as one: until the caller is authorized for
    // this app, a refusal is not the app's to record.
    let org = attempt
        .clock
        .run(Stage::Admit, authorize_refusing_promote(db, &input, target))
        .await?;
    attempt.authorized_org = Some(org.id);
    let admitted = attempt
        .clock
        .run(Stage::Admit, admit(db, &input, target, org))
        .await?;
    // An app that already exists outlives a failure; one this publish creates
    // does not, and is set once the publish stands.
    attempt.app_id = admitted
        .existing_app
        .as_ref()
        .map(|app| app.id)
        .or(admitted.sandbox_app);
    let mut inflated = attempt
        .clock
        .run(Stage::Inflate, inflate(db, &mut input, &admitted))
        .await?;
    let files = std::mem::take(&mut inflated.files);

    let (app_id, is_new_app, rollback) = attempt
        .clock
        .run(
            Stage::Store,
            app_row_for(db, &admitted.org, &input, admitted.sandbox_app),
        )
        .await?;
    let mut guard = OrphanGuard::armed(db, app_id, &input.build_id, rollback);
    let ctx = Publishing {
        db,
        app_id,
        input: &input,
        target,
        org: &admitted.org,
        inflated: &inflated,
        options,
    };
    let (build_pk, warnings, broken) =
        match write(&ctx, &mut guard, &mut attempt.clock, files).await {
            Ok(written) => written,
            Err(e) => {
                guard.undo().await;
                return Err(e);
            }
        };
    guard.disarm();
    // Only now: an app this publish created was undone with a failure.
    attempt.app_id = Some(app_id);
    attempt.app_build_id = Some(build_pk);

    let warnings = attempt
        .clock
        .run_infallible(Stage::After, after(&ctx, build_pk, warnings))
        .await;
    Ok(target.result(
        &admitted.org.slug,
        input,
        (app_id, is_new_app),
        (warnings, broken),
    ))
}

/// Admit's first piece: a sandbox publish that promotes is refused, then the
/// caller is held to the org and app it names.
async fn authorize_refusing_promote(
    db: &DatabaseConnection,
    input: &PublishInput,
    target: &PublishTarget,
) -> Result<organizations::Model, PublishError> {
    target.refuse_promote(input)?;
    authorize(db, input).await
}

/// What every stage after the app row's upsert reads.
struct Publishing<'a> {
    db: &'a DatabaseConnection,
    app_id: Uuid,
    input: &'a PublishInput,
    target: &'a PublishTarget,
    org: &'a organizations::Model,
    inflated: &'a Inflated,
    options: PublishOptions,
}

/// The stages that write, in order, each arming [`OrphanGuard`] as it lands.
/// Answers the build row, the warnings the event gate and the pointer move
/// produced, and the production listeners the publish broke on purpose.
async fn write(
    ctx: &Publishing<'_>,
    guard: &mut OrphanGuard<'_>,
    clock: &mut StageTimer,
    files: Vec<(String, Vec<u8>)>,
) -> Result<(Uuid, Vec<String>, Vec<String>), PublishError> {
    let stored = clock.run(Stage::Store, store_bytes(ctx, files)).await?;
    guard.bytes_stored();
    let build_pk = clock.run(Stage::Record, record(ctx, guard, stored)).await?;
    let gate = production_gate::check(ctx.db, ctx.app_id, build_pk, ctx.input);
    clock.run(Stage::Gate, gate).await?;
    // The event gate once production's blockers pass: the one refusal
    // `--break-listeners` overrides, still before any migration applies.
    let (mut warnings, broken) = clock.run(Stage::Gate, event_gate(ctx, build_pk)).await?;
    clock.run(Stage::Promote, promote(ctx, build_pk)).await?;
    warnings.extend(clock.run(Stage::Point, point(ctx, build_pk)).await?);
    Ok((build_pk, warnings, broken))
}

/// A promoting publish does not take production over a listener it breaks
/// (`app_events::gate`), before anything it would have to undo is applied:
/// the build row is discarded with the guard. `--break-listeners` lets it
/// through, naming what it broke for the audit row; other listeners warn.
async fn event_gate(
    ctx: &Publishing<'_>,
    build_pk: Uuid,
) -> Result<(Vec<String>, Vec<String>), PublishError> {
    if !ctx.input.promote {
        return Ok((Vec::new(), Vec::new()));
    }
    let db_err = |e: sea_orm::DbErr| PublishError::Db(e.to_string());
    let app = entity::apps::Entity::find_by_id(ctx.app_id)
        .one(ctx.db)
        .await
        .map_err(db_err)?;
    let build = entity::app_builds::Entity::find_by_id(build_pk)
        .one(ctx.db)
        .await
        .map_err(db_err)?;
    let (Some(app), Some(build)) = (app, build) else {
        return Ok((Vec::new(), Vec::new()));
    };
    crate::custom_apps_promote::production::event_gate(
        ctx.db,
        &app,
        &build,
        crate::custom_apps_environments::EnvAction::Promote,
        ctx.options.break_listeners,
    )
    .await
    .map_err(|e| match e {
        crate::custom_apps_promote::PromoteError::EventListenersBroken(m) => {
            PublishError::EventListenersBroken(m)
        }
        other => PublishError::Db(other.to_string()),
    })
}

/// Bytes land before the row mutation is allowed to stand. A build id must be
/// unique per app, re-checked here against the row this publish works on (the
/// authoritative check; `admit`'s was the fast path): the store writes key by
/// key into the build's prefix without wiping it, the bundle cache caches
/// per-`build_id` absences on the strength of "a build's file set is fixed
/// once `put_build` returns", and `gc_builds` deletes by `build_id` prefix —
/// so a reused id would merge into the old build, serve broken apps on every
/// replica that already probed it, and have either row's reaping delete the
/// other's bytes. Answers the stored prefix and the environment the build row
/// records.
async fn store_bytes(
    ctx: &Publishing<'_>,
    files: Vec<(String, Vec<u8>)>,
) -> Result<(String, AppEnvironment), PublishError> {
    let input = ctx.input;
    if build_id_taken(ctx.db, ctx.app_id, &input.build_id).await? {
        return Err(PublishError::DuplicateBuild {
            app_slug: input.app_slug.clone(),
            build_id: input.build_id.clone(),
        });
    }
    let s3_prefix = store::put_build(ctx.app_id, &input.build_id, files).await?;
    Ok((s3_prefix, ctx.target.published_to()))
}

/// The build row, then the bundle's functions registered against it so
/// `/fn/<name>` resolves. The `app_functions` FK cascades when the build row
/// is deleted, so a failure registering them is undone by discarding the row.
async fn record(
    ctx: &Publishing<'_>,
    guard: &mut OrphanGuard<'_>,
    stored: (String, AppEnvironment),
) -> Result<Uuid, PublishError> {
    let build_prefix = stored.0.clone();
    let author = sandbox_agent_publish::author(ctx.input);
    let manifest = ctx.inflated.manifest_json.clone();
    let build_pk = record_build(ctx.db, ctx.app_id, ctx.input, stored, manifest, author).await?;
    guard.recorded(build_pk);
    record_functions(
        ctx.db,
        ctx.app_id,
        build_pk,
        &build_prefix,
        &ctx.inflated.fn_specs,
    )
    .await?;
    // One revision per declared event, in every environment.
    crate::app_events::record_revisions(ctx.db, ctx.app_id, ctx.inflated.manifest_json.as_ref())
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?;
    Ok(build_pk)
}

/// Schema ships with the code that needs it — on a PROMOTING publish only,
/// and BEFORE the published pointer moves, so a failed apply leaves the
/// previous build live (a half-migrated app whose code already went live is
/// worse than a promote that did not happen). A draft shares production's
/// `app_<writer>` schema and never applies its DDL. A partial apply is not
/// rolled back: the files that succeeded are in the ledger, so the fixed
/// re-publish resumes.
async fn promote(ctx: &Publishing<'_>, build_pk: Uuid) -> Result<(), PublishError> {
    let (input, app_id) = (ctx.input, ctx.app_id);
    if !input.promote {
        return Ok(());
    }
    let applied = migrations::apply_on_promote(
        ctx.db,
        app_id,
        &input.app_slug,
        ctx.org.id,
        build_pk,
        &ctx.inflated.declared_migrations,
    )
    .await
    .map_err(|e| {
        tracing::warn!("publish: schema migrations failed for app {app_id}: {e}");
        PublishError::Migration(e)
    })?;
    if !applied.summary().is_empty() {
        tracing::info!("publish: {} for app {app_id}", applied.summary());
    }
    // The app's Airhouse tables, after its OLTP schema and under the same rule.
    let applied = migrations::apply_airhouse_on_promote(
        ctx.db,
        app_id,
        &input.app_slug,
        input.project_id,
        build_pk,
        &ctx.inflated.declared_airhouse,
    )
    .await
    .map_err(|e| {
        tracing::warn!("publish: Airhouse migrations failed for app {app_id}: {e}");
        PublishError::Migration(e)
    })?;
    if !applied.summary().is_empty() {
        tracing::info!("publish: Airhouse {} for app {app_id}", applied.summary());
    }
    Ok(())
}

/// Move the pointers, then reconcile the schedules that follow them. Only the
/// move can fail the publish. Production's schedules track the LIVE build, so
/// they are (re)registered only on a PROMOTING publish — a draft must not
/// start firing background runs in production — and after the pointer moved,
/// so a fire resolves the just-set production build. The environment the
/// publish moved outside production is reconciled to its schedules switch. A
/// schedule failure is a warning: the functions stay route-invocable.
async fn point(ctx: &Publishing<'_>, build_pk: Uuid) -> Result<Vec<String>, PublishError> {
    let (input, app_id) = (ctx.input, ctx.app_id);
    move_pointers(ctx.db, ctx.target, app_id, build_pk, input).await?;
    let mut warnings = if input.promote {
        env_schedules::reconcile_production(
            ctx.db,
            app_id,
            input.project_id,
            &ctx.inflated.fn_specs,
        )
        .await
    } else {
        Vec::new()
    };
    if input.promote {
        warnings.extend(production_subscriptions(ctx.db, app_id, build_pk).await);
    }
    warnings.extend(reconcile_moved_environment(ctx.db, ctx.target, app_id).await);
    Ok(warnings)
}

/// Production's event subscriptions, from the build the promoting publish
/// made live (`app_events::subscriptions`). A warning, never a failure.
async fn production_subscriptions(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_pk: Uuid,
) -> Vec<String> {
    let rows = async {
        let app = entity::apps::Entity::find_by_id(app_id).one(db).await?;
        let build = entity::app_builds::Entity::find_by_id(build_pk)
            .one(db)
            .await?;
        Ok::<_, sea_orm::DbErr>(app.zip(build))
    }
    .await;
    match rows {
        Ok(Some((app, build))) => subscriptions::reconcile_production(db, &app, &build).await,
        Ok(None) => Vec::new(),
        Err(e) => vec![format!(
            "production's event subscriptions could not be reconciled ({e}); they keep their \
             previous subscriptions until the next successful publish"
        )],
    }
}

/// Everything after the pointers have moved, none of it able to fail the
/// publish: the publish's own warnings, retention GC, the resolution cache
/// dropped so the new build is visible before the TTL (per-process, like
/// every cache here), the bundle LRU warmed for the engineer's immediate
/// reload, and the homes' migrations queued for the worker fleet — an apply
/// can run for minutes, and a failure is recorded on its run, never on the
/// publish (`custom_apps_nonproduction::staging_task`).
async fn after(ctx: &Publishing<'_>, build_pk: Uuid, mut warnings: Vec<String>) -> Vec<String> {
    let (input, app_id, inflated) = (ctx.input, ctx.app_id, ctx.inflated);
    warnings.extend(inflated.warnings.iter().cloned());
    // Still served, through the one production move, by every CLI that sends
    // it (environments design §11 #4) — with this warning.
    if input.promote {
        warnings.push(PROMOTING_PUBLISH_DEPRECATED.to_string());
    }
    warnings.extend(
        super::calls::warnings(
            ctx.db,
            ctx.org.id,
            &ctx.target.published_to(),
            inflated.manifest_json.as_ref(),
        )
        .await,
    );
    gc_builds(ctx.db, app_id, &[build_pk]).await;
    drop_resolution_cache(ctx.target);
    warm_cache(app_id, input, inflated);
    let built = staging_task::StagingBuild {
        app_id,
        app_slug: &input.app_slug,
        workspace_id: input.project_id,
        org_id: ctx.org.id,
        build_pk,
    };
    warnings.extend(
        queue_migrations_for(
            ctx.db,
            ctx.target,
            built,
            &inflated.declared_migrations,
            &inflated.declared_airhouse,
        )
        .await,
    );
    warnings
}

/// Everything on the critical path of the first visit — the shell, the asset
/// manifest the serve path reads to build preload hints, the entry chunks —
/// so the reload `oxyc publish` invites is not one cold object-store read per
/// entry.
fn warm_cache(app_id: Uuid, input: &PublishInput, inflated: &Inflated) {
    if let Some(bytes) = &inflated.index_bytes {
        cache::seed(app_id, &input.build_id, "index.html", bytes.clone());
    }
    if let Ok(bytes) = serde_json::to_vec(&inflated.asset_manifest) {
        cache::seed(
            app_id,
            &input.build_id,
            asset_manifest::ASSET_MANIFEST_PATH,
            Bytes::from(bytes),
        );
    }
    for (path, bytes) in &inflated.seed_entries {
        cache::seed(app_id, &input.build_id, path, bytes.clone());
    }
}
