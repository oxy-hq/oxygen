//! Staff/partner-facing OLTP surface for the admin console.
//!
//! The operator flow this exists for: pick an org → see whether it has an OLTP
//! database → provision one → get a connection string → iterate. Before this,
//! all of that was `oxy oltp` on someone's laptop, which meant nobody without a
//! checkout and a superuser DSN could do any of it.
//!
//! **These routes are org-keyed, not workspace-keyed.** The member-facing
//! `/oltp/me/*` endpoints resolve the org from the caller's workspace, which is
//! exactly wrong for an operator who is not a member of the org at all.
//!
//! **There is deliberately no `router()` here.** These handlers are mounted by
//! `oxy_app::server::api::admin::oltp`, which calls `scope::deny_out_of_scope`
//! on every org-keyed one before delegating. A router in this crate cannot do
//! that — the fence reads `app_admins` through `server::authz::globals`, which
//! lives above this crate — so one existed and was unfenced, and the belief
//! that authorization was "a route layer, not a check in here" is what produced
//! that gap: `cap(Action::PlatformOltp)` decides on a nil-org resource, so it
//! lets a bounded `global_admin` through for every org on the deployment.
//! `app_scope_boundary.rs` reads the shim, so a `.merge()` of a router declared
//! here would compile, drop the fence, and pass the test.
//!
//! The capability layer still applies at the mount: provisioning creates a
//! billable project, which is why it sits behind `Cap::OperatePlatform` rather
//! than the plain staff door. It is the *narrowing* that has to happen in a
//! handler — see `crates/authz/CLAUDE.md`.

use axum::extract::{Json, Path};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_platform::db::establish_connection;

use super::handlers::{ConnectionInfoResponse, status_for_org};

/// 503 when the deployment simply has no OLTP configured, 500 for a real fault.
///
/// Both design docs promise 503 for this, and the difference is what keeps a
/// monitoring alert off a normal state — an unconfigured deployment is not an
/// Oxy bug, and the console can say so instead of "Could not provision".
fn provisioner_status(e: crate::provisioner::ProvisionerError) -> (StatusCode, String) {
    use crate::provisioner::ProvisionerError as P;
    let code = match &e {
        P::NotConfigured(_) => StatusCode::SERVICE_UNAVAILABLE,
        P::OrgNotFound(_) | P::NotProvisioned(_) => StatusCode::NOT_FOUND,
        // Operator error, not an Oxy fault: a state the caller has to resolve
        // (deprovision first, wait for the tenant to settle, rename a writer).
        // `ProviderMismatch` in particular read as a bug when it is a refusal.
        P::NotActive(..) | P::ProviderMismatch { .. } | P::SchemaNamespaceClaimed { .. } => {
            StatusCode::CONFLICT
        }
        // Also operator-actionable, and it used to fall through to 500: a
        // database sitting where this tenant's belongs is the most resolvable
        // state in this surface, and the console said "Could not provision".
        P::Provider(crate::provider::ProviderError::ProjectNotOwned { .. })
        | P::Provider(crate::provider::ProviderError::ProjectNameTaken(_)) => StatusCode::CONFLICT,
        // Oxy's own refusals, raised as SQLSTATEs from inside a DDL batch:
        // OXY01 (role not confined), OXY02 (schema owned by another role),
        // OXY03 (role owns objects and will not be stripped). Every one is a
        // state an operator resolves, and every one arrived here as a 500
        // carrying a Postgres error — the previous commit claimed preserving
        // the writer loop's status code fixed OXY02 and OXY03, and it did not,
        // because nothing classified them in the first place.
        //
        // Matched on `source_message`, which `pg_detail` prefixes with
        // `[OXY0…]`, NOT on `Display` — `SqlError::Statement`'s Display embeds
        // the statement text, and `assert_confined_sql` carries its own
        // `ERRCODE` literal in that text. `is_unconfined` learned this the
        // expensive way.
        P::Sql(crate::sql::SqlError::Statement { source_message, .. })
            if source_message.starts_with("[OXY0") =>
        {
            StatusCode::CONFLICT
        }
        P::Schema(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

fn internal<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> StatusCode {
    move |e| {
        error!("oltp admin: {what}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// One row of the fleet-wide OLTP list.
#[derive(Debug, Serialize)]
pub struct TenantRow {
    pub org_id: Uuid,
    pub org_name: String,
    pub database: String,
    pub host: String,
    pub provider: String,
    pub region: String,
    pub status: String,
    /// The schemas inside this database, not a count of them.
    ///
    /// The console renders them as a strip of chips, because the count was the
    /// least interesting fact about them: "2" does not say whether analytics
    /// can read the app's live rows, which is the security-relevant one and
    /// fits in the same column width.
    pub schemas: Vec<TenantSchema>,
    pub analyst_ready: bool,
    /// True when the tenant is behind on Oxy's own in-database objects.
    pub platform_drift: bool,
}

/// One schema on the fleet list. Deliberately smaller than the settings
/// panel's `SchemaInfo` — a fleet row shows what a schema IS, not how to
/// connect to it.
#[derive(Debug, Serialize)]
pub struct TenantSchema {
    pub schema: String,
    /// `app` or `pipeline`.
    pub kind: String,
    pub analytics_visible: bool,
}

/// Every org that has an OLTP database, plus every org that does not.
///
/// Both halves matter: the operator question is usually "who still needs one",
/// which a list of existing tenants cannot answer. Orgs without a database come
/// back with an empty `database` and `status: "none"`, so one screen shows the
/// whole fleet.
#[instrument(skip(user), fields(user_id = %user.id))]
pub async fn list_tenants(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<TenantRow>>, StatusCode> {
    use crate::entity::tenants::Entity as OltpTenants;
    use sea_orm::EntityTrait;
    use std::collections::HashMap;

    let db = establish_connection().await.map_err(internal("connect"))?;

    let orgs = entity::prelude::Organizations::find()
        .all(&db)
        .await
        .map_err(internal("list orgs"))?;
    let tenants = OltpTenants::find()
        .all(&db)
        .await
        .map_err(internal("list oltp tenants"))?;

    // ONE query for every tenant's roles, grouped in memory.
    //
    // This counted per tenant inside the loop below, so a deployment with 200
    // provisioned orgs issued 201 queries to render one page — and the count it
    // paid for was thrown away in favour of the schema names anyway.
    let mut by_tenant: HashMap<uuid::Uuid, Vec<TenantSchema>> = HashMap::new();
    for role in crate::entity::roles::Entity::find()
        .all(&db)
        .await
        .map_err(internal("list oltp roles"))?
    {
        let kind = match role.writer_kind {
            crate::entity::roles::WriterKind::App => "app",
            crate::entity::roles::WriterKind::Pipeline => "pipeline",
        };
        by_tenant
            .entry(role.tenant_row_id)
            .or_default()
            .push(TenantSchema {
                analytics_visible: crate::migrator::effective_visibility(
                    role.analytics_visible,
                    &role.writer_kind,
                ),
                schema: role.schema_name,
                kind: kind.to_string(),
            });
    }

    let mut rows = Vec::with_capacity(orgs.len());
    for org in orgs {
        let tenant = tenants.iter().find(|t| t.org_id == org.id);
        let row = match tenant {
            None => TenantRow {
                org_id: org.id,
                org_name: org.name,
                database: String::new(),
                host: String::new(),
                provider: String::new(),
                region: String::new(),
                status: "none".to_string(),
                schemas: Vec::new(),
                analyst_ready: false,
                platform_drift: false,
            },
            Some(t) => {
                let mut schemas = by_tenant.remove(&t.id).unwrap_or_default();
                schemas.sort_by(|a, b| a.schema.cmp(&b.schema));
                TenantRow {
                    org_id: org.id,
                    org_name: org.name,
                    database: t.database_name.clone(),
                    host: t.host.clone(),
                    provider: t.provider.clone(),
                    region: t.region.clone(),
                    status: t.status.as_str().to_string(),
                    schemas,
                    analyst_ready: t.analyst_password_ciphertext.is_some(),
                    platform_drift: t.platform_schema_version
                        != crate::platform::PLATFORM_SCHEMA_VERSION,
                }
            }
        };
        rows.push(row);
    }

    // Provisioned first, then alphabetical: the rows an operator acts on are
    // the ones with a database behind them.
    rows.sort_by(|a, b| {
        (a.status == "none")
            .cmp(&(b.status == "none"))
            .then(a.org_name.cmp(&b.org_name))
    });
    Ok(Json(rows))
}

/// What the console shows for an org, provisioned or not. No credentials.
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn get_status(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<ConnectionInfoResponse>, StatusCode> {
    let db = establish_connection().await.map_err(internal("connect"))?;
    Ok(Json(status_for_org(&db, org_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct ProvisionRequest {
    /// Writers to create alongside the database, as `app:<slug>` /
    /// `pipeline:<source>`. Optional — an operator can provision the database
    /// first and add writers when an app actually exists.
    #[serde(default)]
    pub writers: Vec<String>,
}

/// Provision (or reconcile) the org's database.
///
/// Idempotent, because [`OltpProvisioner::provision`] is: a double-click, a
/// retry after a timeout, or two operators racing all converge on one database
/// rather than two. That matters more here than on the CLI — a button invites
/// exactly those.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn provision(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<ProvisionRequest>,
) -> Result<Json<ConnectionInfoResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Parse every writer BEFORE provisioning: a typo should cost nothing, not
    // leave a database created and half its schemas missing.
    let writers = body
        .writers
        .iter()
        .map(|spec| parse_writer(spec))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let provisioner = crate::provisioner::from_env(db.clone())
        .await
        .map_err(provisioner_status)?;

    info!(user = %user.label(), org_id = %org_id, writers = writers.len(), "provisioning OLTP");
    // `provisioner_status`, not a hardcoded 500.
    //
    // This closure is why the CONFLICT arm existed and never fired: every
    // refusal `provision` can make — a database that is not ours, a name
    // collision, a tenant mid-deprovision — reached the console as
    // "Could not provision" with a 500, which reads as an Oxy fault for the
    // most operator-actionable states in this surface.
    provisioner
        .provision(org_id)
        .await
        .map_err(provisioner_status)?;

    for writer in &writers {
        provisioner
            .ensure_writer(
                org_id,
                writer,
                crate::schema::GrantLevel::ReadWrite,
                // Claimed by no workspace: an operator provisioning ahead of an
                // app must not lock the namespace to a workspace that has not
                // declared it yet. `None`, not `Uuid::nil()` — nil compares as
                // an ordinary workspace id, so it locked the namespace to one
                // that will never exist.
                None,
            )
            .await
            // Prefixes the writer onto the message and KEEPS the status code.
            // Replacing it was the bug: OXY02 (a schema owned by another role)
            // and OXY03 (a role owning objects) are states an operator
            // resolves, and both read as an Oxy fault because this arm chose
            // the code itself.
            //
            // Not `SchemaNamespaceClaimed`, which this loop cannot raise: it
            // passes `claimant: None`, and `claim_namespace` sends
            // `(Some(_), None)` to `Ok(())` — only two REAL workspaces collide.
            // It reaches `provisioner_status` from the CLI and the compile
            // path.
            .map_err(|e| {
                let (code, message) = provisioner_status(e);
                (code, format!("{writer}: {message}"))
            })?;
    }

    let db2 = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let status = status_for_org(&db2, org_id)
        .await
        .map_err(|s| (s, "could not read back status".to_string()))?;
    Ok(Json(status))
}

#[derive(Debug, Deserialize)]
pub struct VisibilityRequest {
    /// `app:<slug>` or `pipeline:<source>`.
    pub writer: String,
    pub visible: bool,
}

/// Let the read-only analyst read a writer's schema, or withdraw it.
///
/// A separate verb from provisioning on purpose: widening who can read a
/// tenant's application data is its own decision, and `app_*` is hidden by
/// default precisely because live app state may be regulated.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn set_visibility(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<VisibilityRequest>,
) -> Result<Json<ConnectionInfoResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let writer = parse_writer(&body.writer).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let provisioner = crate::provisioner::from_env(db.clone())
        .await
        .map_err(provisioner_status)?;

    info!(user = %user.label(), writer = %writer, visible = body.visible, "analytics visibility");
    provisioner
        .set_analytics_visibility(org_id, &writer, body.visible)
        .await
        .map_err(provisioner_status)?;

    let db2 = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    status_for_org(&db2, org_id)
        .await
        .map(Json)
        .map_err(|s| (s, "could not read back status".to_string()))
}

/// Destroy the org's database at the provider.
///
/// Irreversible, and on a managed provider it deletes a real billing resource —
/// so the UI asks for the database name back before it calls this, the same way
/// the CLI demands `--yes`.
#[instrument(skip(user), fields(user_id = %user.id, org_id = %org_id))]
pub async fn deprovision(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<ConnectionInfoResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let provisioner = crate::provisioner::from_env(db.clone())
        .await
        .map_err(provisioner_status)?;

    warn!(user = %user.label(), org_id = %org_id, "DEPROVISIONING an OLTP database");
    provisioner
        .deprovision(org_id)
        .await
        .map_err(provisioner_status)?;

    let db2 = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    status_for_org(&db2, org_id)
        .await
        .map(Json)
        .map_err(|s| (s, "could not read back status".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct DeprovisionWriterRequest {
    /// `app:<slug>` or `pipeline:<source>`.
    pub writer: String,
}

/// Drop ONE writer's schema and role, leaving the rest of the tenant intact.
///
/// # Why this route exists
///
/// [`OltpProvisioner::deprovision_writer`] has always been the escape hatch the
/// app-delete and app-rename guards point at — its own doc says it is "what
/// makes the app-delete and rename guards actionable". But its only caller was
/// `oxy oltp deprovision --writer`, and that subcommand has no `--env`: it
/// reads `OXY_DATABASE_URL` and talks to the LOCAL control plane. So on every
/// cloud environment the guard refused an app delete, named a remedy, and the
/// remedy could not reach the environment that refused. The only route that
/// could was `DELETE /oltp`, which destroys the whole tenant database and every
/// other app's data with it.
///
/// That is the shape of a guard that reads as correct and functions as a dead
/// end. This is the door.
///
/// A POST rather than a DELETE, matching `provision` / `visibility` /
/// `credentials` on this surface — and deliberately NOT sharing a path with the
/// whole-tenant `DELETE .../oltp`. These two operations differ by one word and
/// by the entire tenant; making them differ by an HTTP verb on one URL is how
/// somebody destroys a database meaning to release one app.
///
/// Idempotent, because the provisioner is: a writer that was never provisioned
/// is a no-op, so an operator retrying a wedged app delete does not have to
/// know how far the last attempt got.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn deprovision_writer(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<DeprovisionWriterRequest>,
) -> Result<Json<ConnectionInfoResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // Parsed before the provisioner is built, so a malformed spec is a 400 and
    // not a 500 from somewhere deeper. `app:my-app` is rejected here on purpose:
    // a slug's hyphens are illegal in a writer name, and the guard's message
    // hands over the DERIVED writer for exactly that reason.
    let writer = parse_writer(&body.writer).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let provisioner = crate::provisioner::from_env(db.clone())
        .await
        .map_err(provisioner_status)?;

    warn!(
        user = %user.label(),
        org_id = %org_id,
        writer = %writer,
        "DEPROVISIONING one OLTP writer — its schema and data go with it"
    );
    provisioner
        .deprovision_writer(org_id, &writer)
        .await
        .map_err(provisioner_status)?;

    let db2 = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    status_for_org(&db2, org_id)
        .await
        .map(Json)
        .map_err(|s| (s, "could not read back status".to_string()))
}

fn parse_writer(spec: &str) -> Result<crate::schema::WriterRef, String> {
    let (kind, name) = spec.split_once(':').ok_or_else(|| {
        format!("writer {spec:?} must look like `app:<slug>` or `pipeline:<src>`")
    })?;
    match kind {
        "app" => crate::schema::WriterRef::app(name),
        "pipeline" => crate::schema::WriterRef::pipeline(name),
        _ => {
            return Err(format!(
                "writer {spec:?} must start with `app:` or `pipeline:`"
            ));
        }
    }
    .map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
pub struct CredentialsRequest {
    /// `analyst` (read-only) or a writer spec like `app:bookings`.
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct CredentialsResponse {
    pub role: String,
    pub dsn: String,
    /// Whether this DSN can write. The UI leads with it, because the whole
    /// design rests on humans normally holding a read-only one.
    pub writable: bool,
}

/// Hand an operator a DSN they can paste into `psql` and iterate with.
///
/// **POST, not GET, and logged at `warn`.** Disclosing a live credential is an
/// event, not a read: it must not land in a browser history, a proxy log, or a
/// prefetch, and it should be visible afterwards in the operator's own logs.
///
/// `analyst` is read-only and is what the UI offers first — it is enough to
/// inspect a schema, which is what "let me look at the database" almost always
/// means. A writer DSN is a real write credential to one app's schema, and is
/// the one that needs the warning.
#[instrument(skip(user, body), fields(user_id = %user.id, org_id = %org_id))]
pub async fn credentials(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CredentialsRequest>,
) -> Result<Json<CredentialsResponse>, (StatusCode, String)> {
    let db = establish_connection()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if body.role == "analyst" {
        let conn = crate::resolver::resolve_analyst_connection_for_org(&db, org_id)
            .await
            .map_err(super::resolve_status)?;
        warn!(
            user = %user.label(), org_id = %org_id,
            "disclosed the read-only OLTP analyst credential"
        );
        return Ok(Json(CredentialsResponse {
            // `conn.user`, NOT the bare `ANALYST_ROLE` constant.
            //
            // The resolver sets `user` to `analyst_role_for(provider,
            // database)`, which on a shared-namespace provider is
            // `oxy_analyst_ro_<tag>` — so the panel showed `oxy_analyst_ro`
            // beside a DSN authenticating as something else, and an operator
            // who copied the username into their own `psql` got an auth
            // failure against a DSN that works. Last residue of the decoy-
            // analyst bug, and the one place that hands out a real credential.
            role: conn.user.clone(),
            dsn: conn.dsn(),
            writable: false,
        }));
    }

    let writer = parse_writer(&body.role).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let conn = crate::resolver::resolve_writer_connection_for_org(&db, org_id, &writer)
        .await
        .map_err(super::resolve_status)?;
    warn!(
        user = %user.label(), org_id = %org_id, writer = %writer,
        "disclosed a WRITE credential to an OLTP schema"
    );
    Ok(Json(CredentialsResponse {
        role: conn.role.clone(),
        dsn: conn.dsn.clone(),
        writable: true,
    }))
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use crate::provisioner::ProvisionerError as P;

    fn statement_err(source_message: &str) -> P {
        P::Sql(crate::sql::SqlError::Statement {
            // The real shape: `assert_confined_sql` carries its own `ERRCODE`
            // literal in the statement text, which is why the arm must not
            // match on `Display`.
            statement: "DO $$ BEGIN RAISE EXCEPTION 'x' USING ERRCODE = 'OXY02'; END $$"
                .to_string(),
            source_message: source_message.to_string(),
        })
    }

    /// Oxy's own refusals must reach the console as 409, not 500.
    ///
    /// These are raised as SQLSTATEs from inside a DDL batch, so they arrive as
    /// `Sql(Statement { .. })` and fell to the wildcard — a state the operator
    /// resolves, rendered as an Oxy fault. The previous commit claimed
    /// preserving the writer loop's status code fixed OXY02 and OXY03; nothing
    /// classified them, so it did not.
    #[test]
    fn oxy_sqlstates_are_conflicts() {
        for code in ["OXY01", "OXY02", "OXY03"] {
            let (status, _) = provisioner_status(statement_err(&format!(
                "[{code}] something the operator must resolve"
            )));
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "{code} must be operator-actionable, not a 500"
            );
        }
    }

    /// And an ordinary SQL failure stays a 500.
    ///
    /// The arm keys on the `[OXY0` prefix `pg_detail` writes, so a real
    /// Postgres error must not be swept into 409 with it.
    #[test]
    fn an_ordinary_sql_failure_is_still_a_server_error() {
        let (status, _) = provisioner_status(statement_err(
            "[42501] permission denied for database oxy_org_x",
        ));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// Matching must not read `Display`, which embeds the statement text.
    ///
    /// `SqlError::Statement`'s Display is `statement failed ({statement}):
    /// {source_message}`, and the statement carries `ERRCODE = 'OXY02'` — so a
    /// `to_string().contains("OXY02")` test would classify an unrelated failure
    /// of that same statement as a conflict. This is `is_unconfined`'s lesson,
    /// pinned here too.
    #[test]
    fn the_statement_text_alone_does_not_make_it_a_conflict() {
        let (status, _) = provisioner_status(statement_err("[40001] serialization failure"));
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "the ERRCODE literal inside the statement must not classify it"
        );
    }
}
