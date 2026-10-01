//! Neon REST API client — the real provider behind [`OltpProvider`].
//!
//! Wire contract taken from Neon's published OpenAPI spec
//! (<https://neon.tech/api_spec/release/v2.json>), base
//! `https://console.neon.tech/api/v2`, bearer-authenticated with an API key.
//!
//! Two things about this API shape the client more than the endpoint list does:
//!
//! 1. **Writes are asynchronous.** A create/delete returns `202`-ish semantics
//!    with an `operations[]` array; the compute is not reachable until those
//!    reach `finished`. Returning as soon as the HTTP call succeeds would hand
//!    the caller a DSN that refuses connections for the next few seconds — and
//!    [`crate::provisioner`] connects immediately to run DDL. So every mutating
//!    call polls its operations before returning.
//!
//! 2. **A password is disclosed once.** `POST /roles` and
//!    `.../reset_password` return it; nothing else ever does. Whether a plain
//!    `GET` can see it depends on the project's `store_passwords` setting, so
//!    this client never relies on it — [`OltpProvider::get_role`] deliberately
//!    reports `password: None` regardless of what the wire said, matching the
//!    trait contract and keeping "lost password ⇒ reset it" true everywhere.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::types::{
    Branch, BranchRequest, CreateProjectRequest, DatabaseInfo, Project, ProjectBranch, Role,
};
use super::{OltpProvider, ProviderError};

// The staging branch's half of the client. A child module so it reaches
// `send` / `await_operations` without widening them.
mod branches;

const DEFAULT_BASE_URL: &str = "https://console.neon.tech/api/v2";

/// How long to wait for a project's compute to finish provisioning.
///
/// Neon's own guidance is seconds; this is the ceiling before we call it a
/// failure rather than a normal wait.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const OPERATION_POLL_INTERVAL: Duration = Duration::from_millis(750);

/// Per-request ceiling. Generous: project creation is genuinely slow.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub struct NeonProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Neon organization that owns every project Oxy creates. Without it the
    /// projects land under the API key's personal account, where per-org
    /// billing and access control do not apply.
    org_id: String,
}

impl NeonProvider {
    pub fn new(api_key: impl Into<String>, org_id: impl Into<String>) -> Self {
        Self::with_base_url(api_key, org_id, DEFAULT_BASE_URL)
    }

    /// Point the client at a different base URL. Exists so tests can drive a
    /// local stub server over the real transport rather than a hand-rolled fake.
    pub fn with_base_url(
        api_key: impl Into<String>,
        org_id: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client with a timeout is always constructible"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            org_id: org_id.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Issue one request and classify the outcome.
    ///
    /// `None` is returned for `404`, so callers can express "absent" without
    /// treating it as an error — which is what makes `get_*` return
    /// `Ok(None)` and `delete_*` idempotent, per the trait contract.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ProviderError> {
        let mut req = self
            .http
            .request(method, self.url(path))
            .bearer_auth(&self.api_key);
        if let Some(body) = body {
            req = req.json(&body);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if status.as_u16() == 404 {
            return Ok(None);
        }
        if status.as_u16() == 429 {
            return Err(ProviderError::RateLimited);
        }
        if !status.is_success() {
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: error_message(&text),
            });
        }
        if text.trim().is_empty() {
            // `DELETE` may answer 204 with no body.
            return Ok(Some(serde_json::Value::Null));
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| ProviderError::Api {
                status: status.as_u16(),
                message: format!("response was not the JSON this client expects: {e}"),
            })
    }

    /// Block until every operation in `body.operations` has settled.
    ///
    /// Neon reports a create as accepted long before the compute can take a
    /// connection. Skipping this makes provisioning fail intermittently, and
    /// only under load — the worst possible way to learn about it.
    async fn await_operations(
        &self,
        project_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), ProviderError> {
        let ids: Vec<String> = body
            .get("operations")
            .and_then(|o| o.as_array())
            .map(|ops| {
                ops.iter()
                    .filter_map(|op| op.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let deadline = std::time::Instant::now() + OPERATION_TIMEOUT;
        for id in ids {
            loop {
                let path = format!("/projects/{project_id}/operations/{id}");
                // A 404 here means the operation record has already aged out,
                // which only happens well after it finished.
                let Some(body) = self.send(reqwest::Method::GET, &path, None).await? else {
                    break;
                };
                let status = body
                    .get("operation")
                    .and_then(|o| o.get("status"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("finished");

                match status {
                    "finished" | "skipped" => break,
                    "failed" | "error" | "cancelled" => {
                        let detail = body
                            .get("operation")
                            .and_then(|o| o.get("error"))
                            .and_then(|e| e.as_str())
                            .unwrap_or("no detail reported");
                        return Err(ProviderError::Api {
                            status: 500,
                            message: format!("operation {id} {status}: {detail}"),
                        });
                    }
                    // scheduling | running | cancelling
                    _ => {
                        if std::time::Instant::now() >= deadline {
                            return Err(ProviderError::Api {
                                status: 504,
                                message: format!(
                                    "operation {id} still {status} after {}s",
                                    OPERATION_TIMEOUT.as_secs()
                                ),
                            });
                        }
                        tokio::time::sleep(OPERATION_POLL_INTERVAL).await;
                    }
                }
            }
        }
        Ok(())
    }
}

impl NeonProvider {
    /// Find a project this Oxy instance already created, by its exact name.
    ///
    /// Neon does **not** enforce unique project names — the spec says an
    /// unnamed project is simply named after its generated id. So a duplicate
    /// create is not rejected, it silently produces a second billable project.
    /// That turns the ordinary crash window in `OltpProvisioner::create_new`
    /// (project created, then the row insert fails) into a leak that an
    /// unattended retry loop compounds.
    ///
    /// Adoption is safe *here* specifically because the name is
    /// `oxy-org-<org uuid>`, derived rather than chosen, and the search is
    /// scoped to Oxy's own Neon org — so a match is an identity match, not a
    /// coincidence. The `airhouse` incident this crate's `ProjectNameTaken`
    /// guards against was the opposite case: user-chosen names, where two
    /// tenants could collide and adoption would have crossed tenants.
    async fn find_project_id_by_name(&self, name: &str) -> Result<Option<String>, ProviderError> {
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!(
                "/projects?org_id={}&search={}&limit=400",
                urlencode(&self.org_id),
                urlencode(name)
            );
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={}", urlencode(c)));
            }
            let Some(body) = self.send(reqwest::Method::GET, &path, None).await? else {
                return Ok(None);
            };
            let projects = body
                .get("projects")
                .and_then(|p| p.as_array())
                .cloned()
                .unwrap_or_default();
            if projects.is_empty() {
                return Ok(None);
            }
            // `search` matches partial names and ids, so an exact comparison is
            // required — `oxy-org-<uuid>` is a prefix of nothing else today, but
            // relying on that would make a future name scheme silently adopt the
            // wrong project.
            if let Some(found) = projects
                .iter()
                .find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
            {
                return Ok(found.get("id").and_then(|i| i.as_str()).map(String::from));
            }
            cursor = body
                .get("pagination")
                .and_then(|p| p.get("cursor"))
                .and_then(|c| c.as_str())
                .map(String::from);
            if cursor.is_none() {
                return Ok(None);
            }
        }
    }
}

/// Percent-encode a query-string value. Project names are `oxy-org-<uuid>` so
/// nothing needs escaping today; this exists so that a future name scheme
/// cannot turn into a malformed URL or a query-parameter injection.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Pull Neon's `message` out of an error body, falling back to the raw text.
fn error_message(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            if text.trim().is_empty() {
                "no response body".to_string()
            } else {
                text.chars().take(300).collect()
            }
        })
}

/// Neon signals a name collision through the message, not a distinct code, so
/// this is a text match by necessity. Getting it wrong is safe in the direction
/// that matters: an unrecognised message stays a generic API error, and the
/// caller never silently adopts somebody else's project.
fn is_name_taken(status: u16, message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    matches!(status, 409 | 422) && (m.contains("already exists") || m.contains("already in use"))
}

// ---------------------------------------------------------------------------
// Wire shapes. Only the fields Oxy uses; Neon returns far more.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WireProject {
    id: String,
    name: String,
    region_id: String,
    pg_version: u8,
}

#[derive(Debug, Deserialize)]
struct WireBranch {
    id: String,
    name: String,
    /// Absent on a project's root branch. Read on adoption: only a branch cut
    /// from production is the one Oxy made.
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireDatabase {
    name: String,
    owner_name: String,
}

#[derive(Debug, Deserialize)]
struct WireRole {
    name: String,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireEndpoint {
    host: String,
    #[serde(default)]
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreatedProject {
    project: WireProject,
    branch: WireBranch,
    #[serde(default)]
    databases: Vec<WireDatabase>,
    #[serde(default)]
    roles: Vec<WireRole>,
    #[serde(default)]
    endpoints: Vec<WireEndpoint>,
}

/// The host a client actually connects to.
///
/// `project.proxy_host` is the region-wide proxy, not this project's endpoint —
/// using it yields a DSN that resolves but authenticates against nothing. The
/// read-write endpoint is the correct one, and a project may also carry
/// read-only replicas, so the type is checked rather than taking `[0]`.
fn read_write_host(endpoints: &[WireEndpoint]) -> Option<String> {
    endpoints
        .iter()
        .find(|e| e.r#type.as_deref() == Some("read_write"))
        .or_else(|| endpoints.first())
        .map(|e| e.host.clone())
}

#[async_trait]
impl OltpProvider for NeonProvider {
    fn name(&self) -> &'static str {
        "neon"
    }

    async fn create_project(&self, req: CreateProjectRequest) -> Result<Project, ProviderError> {
        // `branch.role_name` / `branch.database_name` name the objects Neon
        // creates with the project. Setting them explicitly means the owner role
        // is deterministic instead of Neon's generated default, which is what
        // lets a half-finished provision be reconciled rather than duplicated.
        let owner_role = super::OWNER_ROLE;

        // Recover an orphan from a previous attempt rather than paying for a
        // second project. See `find_project_id_by_name` for why adopting is
        // safe for these names and was not for airhouse's.
        //
        // The lookup runs for EVERY name; only the ANSWER depends on derivation.
        //
        // Gating the lookup itself was wrong, and worse than what it replaced.
        // Neon does not enforce unique project names — `find_project_id_by_name`
        // says so — so skipping the search for a chosen name does not refuse it,
        // it silently creates a SECOND billable project. That made three
        // different answers for one input across two commits: Local refuses,
        // Neon used to adopt, Neon then duplicated. "Must not adopt" and "must
        // create" are not the same requirement, and the third option is the
        // refusal `ProjectNameTaken` exists for — which is what the sibling
        // provider does.
        if let Some(existing_id) = self.find_project_id_by_name(&req.name).await? {
            if !crate::provisioner::is_derived_project_name(&req.name) {
                // Same answer as `LocalProvider`: a chosen name that already
                // exists is a collision, never an adoption. Adoption resets the
                // owner password and takes the project over, which is only safe
                // when the name identifies one org.
                return Err(ProviderError::ProjectNameTaken(req.name));
            }
            tracing::warn!(
                project_id = %existing_id,
                name = %req.name,
                "adopting an existing Neon project — a previous provision created it \
                 but did not record it locally"
            );
            let mut project = self
                .get_project(&existing_id)
                .await?
                .ok_or_else(|| ProviderError::ProjectNotFound(existing_id.clone()))?;
            // Its password was disclosed only to the attempt that lost it, so
            // the only way back to a usable credential is a reset.
            project.owner_role = self
                .reset_role_password(&existing_id, &project.branch.id, &project.owner_role.name)
                .await?;
            return Ok(project);
        }

        let body = serde_json::json!({
            "project": {
                "name": req.name,
                "region_id": req.region_id,
                "pg_version": req.pg_version,
                "org_id": self.org_id,
                "branch": {
                    "role_name": owner_role,
                    "database_name": super::DEFAULT_DATABASE,
                },
            }
        });

        let raw = match self
            .send(reqwest::Method::POST, "/projects", Some(body))
            .await
        {
            Ok(Some(v)) => v,
            Ok(None) => {
                return Err(ProviderError::Api {
                    status: 404,
                    message: "POST /projects returned 404".into(),
                });
            }
            Err(ProviderError::Api { status, message }) if is_name_taken(status, &message) => {
                return Err(ProviderError::ProjectNameTaken(req.name));
            }
            Err(e) => return Err(e),
        };

        let created: CreatedProject =
            serde_json::from_value(raw.clone()).map_err(|e| ProviderError::Api {
                status: 200,
                message: format!("unexpected create-project response: {e}"),
            })?;

        self.await_operations(&created.project.id, &raw).await?;

        let database = created
            .databases
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Api {
                status: 200,
                message: "create-project returned no database".into(),
            })?;
        let role = created
            .roles
            .into_iter()
            .find(|r| r.name == owner_role)
            .ok_or_else(|| ProviderError::Api {
                status: 200,
                message: format!("create-project returned no {owner_role} role"),
            })?;
        if role.password.is_none() {
            // The one moment the password is disclosed. If it is absent there is
            // nothing to recover — better to fail the provision loudly than to
            // record a tenant whose owner credential is unknown.
            return Err(ProviderError::Api {
                status: 200,
                message: "create-project disclosed no owner password".into(),
            });
        }
        let host = read_write_host(&created.endpoints).ok_or_else(|| ProviderError::Api {
            status: 200,
            message: "create-project returned no endpoint to connect to".into(),
        })?;

        Ok(Project {
            id: created.project.id,
            name: created.project.name,
            region_id: created.project.region_id,
            pg_version: created.project.pg_version,
            branch: Branch {
                id: created.branch.id,
                name: created.branch.name,
            },
            database: DatabaseInfo {
                name: database.name,
                owner_name: database.owner_name,
            },
            owner_role: Role {
                name: role.name,
                password: role.password,
            },
            host,
        })
    }

    async fn get_project(&self, project_id: &str) -> Result<Option<Project>, ProviderError> {
        let path = format!("/projects/{project_id}");
        let Some(raw) = self.send(reqwest::Method::GET, &path, None).await? else {
            return Ok(None);
        };

        #[derive(Deserialize)]
        struct Resp {
            project: WireProject,
        }
        let resp: Resp = serde_json::from_value(raw).map_err(|e| ProviderError::Api {
            status: 200,
            message: format!("unexpected get-project response: {e}"),
        })?;

        // `GET /projects/{id}` describes the project only. Branch, database and
        // endpoint each need their own call, so they are fetched here rather
        // than invented — a Project with a blank host would produce a DSN that
        // fails at connect time, far from the cause.
        let branches = self
            .send(
                reqwest::Method::GET,
                &format!("/projects/{project_id}/branches"),
                None,
            )
            .await?
            .unwrap_or(serde_json::Value::Null);
        let branch = branches
            .get("branches")
            .and_then(|b| b.as_array())
            .and_then(|bs| {
                bs.iter()
                    .find(|b| {
                        b.get("default").and_then(|d| d.as_bool()).unwrap_or(false)
                            || b.get("primary").and_then(|d| d.as_bool()).unwrap_or(false)
                    })
                    .or_else(|| bs.first())
            })
            .ok_or_else(|| ProviderError::Api {
                status: 200,
                message: "project has no branches".into(),
            })?;
        let branch_id = branch
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let branch_name = branch
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("main")
            .to_string();

        let dbs = self
            .send(
                reqwest::Method::GET,
                &format!("/projects/{project_id}/branches/{branch_id}/databases"),
                None,
            )
            .await?
            .unwrap_or(serde_json::Value::Null);
        let database = dbs
            .get("databases")
            .and_then(|d| d.as_array())
            .and_then(|ds| ds.first())
            .ok_or_else(|| ProviderError::Api {
                status: 200,
                message: "branch has no databases".into(),
            })?;

        let eps = self
            .send(
                reqwest::Method::GET,
                &format!("/projects/{project_id}/endpoints"),
                None,
            )
            .await?
            .unwrap_or(serde_json::Value::Null);
        let endpoints: Vec<WireEndpoint> = eps
            .get("endpoints")
            .and_then(|e| serde_json::from_value(e.clone()).ok())
            .unwrap_or_default();
        let host = read_write_host(&endpoints).ok_or_else(|| ProviderError::Api {
            status: 200,
            message: "project has no endpoint to connect to".into(),
        })?;

        let owner_name = database
            .get("owner_name")
            .and_then(|v| v.as_str())
            .unwrap_or(super::OWNER_ROLE)
            .to_string();

        Ok(Some(Project {
            id: resp.project.id,
            name: resp.project.name,
            region_id: resp.project.region_id,
            pg_version: resp.project.pg_version,
            branch: Branch {
                id: branch_id,
                name: branch_name,
            },
            database: DatabaseInfo {
                name: database
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                owner_name: owner_name.clone(),
            },
            // Reads never disclose a password; the caller holds the sealed one.
            owner_role: Role {
                name: owner_name,
                password: None,
            },
            host,
        }))
    }

    async fn delete_project(&self, project_id: &str) -> Result<(), ProviderError> {
        let path = format!("/projects/{project_id}");
        // `None` is a 404 — already gone, which the trait defines as success.
        self.send(reqwest::Method::DELETE, &path, None).await?;
        Ok(())
    }

    async fn create_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        let path = format!("/projects/{project_id}/branches/{branch_id}/roles");
        let body = serde_json::json!({ "role": { "name": role_name } });
        let raw = self
            .send(reqwest::Method::POST, &path, Some(body))
            .await?
            .ok_or_else(|| ProviderError::Api {
                status: 404,
                message: format!("branch {branch_id} not found"),
            })?;
        self.await_operations(project_id, &raw).await?;

        let role = parse_role(&raw)?;
        if role.password.is_none() {
            return Err(ProviderError::Api {
                status: 200,
                message: format!("create-role disclosed no password for {role_name}"),
            });
        }
        Ok(role)
    }

    async fn get_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Option<Role>, ProviderError> {
        let path = format!("/projects/{project_id}/branches/{branch_id}/roles/{role_name}");
        let Some(raw) = self.send(reqwest::Method::GET, &path, None).await? else {
            return Ok(None);
        };
        // Password dropped even when `store_passwords` made Neon return it: the
        // trait promises reads never disclose one, and callers that trusted a
        // sometimes-present field would break on the projects where it is off.
        Ok(Some(parse_role(&raw)?.redacted()))
    }

    async fn reset_role_password(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        let path =
            format!("/projects/{project_id}/branches/{branch_id}/roles/{role_name}/reset_password");
        let raw = self
            .send(reqwest::Method::POST, &path, None)
            .await?
            .ok_or_else(|| {
                ProviderError::RoleNotFound(role_name.to_string(), branch_id.to_string())
            })?;
        self.await_operations(project_id, &raw).await?;

        let role = parse_role(&raw)?;
        if role.password.is_none() {
            return Err(ProviderError::Api {
                status: 200,
                message: format!("reset_password disclosed no new password for {role_name}"),
            });
        }
        Ok(role)
    }

    async fn delete_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<(), ProviderError> {
        let path = format!("/projects/{project_id}/branches/{branch_id}/roles/{role_name}");
        if let Some(raw) = self.send(reqwest::Method::DELETE, &path, None).await? {
            self.await_operations(project_id, &raw).await?;
        }
        Ok(())
    }

    async fn create_branch(&self, req: &BranchRequest) -> Result<ProjectBranch, ProviderError> {
        self.create_branch_impl(req).await
    }

    async fn reset_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        self.reset_branch_impl(req, branch_id).await
    }

    async fn delete_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        self.delete_branch_impl(req, branch_id).await
    }

    /// A branch lives inside its project on Neon.
    fn project_delete_takes_branches(&self) -> bool {
        true
    }
}

fn parse_role(raw: &serde_json::Value) -> Result<Role, ProviderError> {
    let wire: WireRole = raw
        .get("role")
        .cloned()
        .ok_or_else(|| ProviderError::Api {
            status: 200,
            message: "response carried no role".into(),
        })
        .and_then(|v| {
            serde_json::from_value(v).map_err(|e| ProviderError::Api {
                status: 200,
                message: format!("unexpected role shape: {e}"),
            })
        })?;
    Ok(Role {
        name: wire.name,
        password: wire.password,
    })
}

#[cfg(test)]
mod stub_tests {
    //! Drives the client over real HTTP against a stub speaking Neon's actual
    //! response shapes.
    //!
    //! A hand-rolled fake of `send()` would assert this client agrees with
    //! itself. What can actually break is the wire contract — a field Neon
    //! nests one level deeper than assumed, a 404 that should mean "absent",
    //! the operations poll — and none of that is visible without a transport.

    use super::*;
    use axum::{Router, extract::State, http::StatusCode, routing};
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct StubState {
        /// How many times the operation was polled — proves the client waited
        /// rather than assuming readiness.
        polls: AtomicUsize,
        /// Operation status flips to `finished` after this many polls.
        finish_after: usize,
        fail_operation: bool,
        /// How many projects the client actually asked Neon to create.
        creates: AtomicUsize,
        /// Whether `GET /projects` reports an already-existing project, i.e.
        /// the orphan a previous provision left behind.
        orphan_exists: bool,
        /// The NAME that orphan carries.
        ///
        /// Configurable because the adoption guard branches on the name, so a
        /// stub that always reports a derived one can only ever exercise the
        /// adopt path: `find_project_id_by_name` matches on name, so a test
        /// passing anything else gets `None` and never enters the guard at all.
        /// That is how the refusal branch came to have a test that could not
        /// reach it.
        orphan_name: Option<String>,
    }

    impl StubState {
        fn orphan_named(name: &str) -> Self {
            Self {
                orphan_exists: true,
                orphan_name: Some(name.to_string()),
                ..Default::default()
            }
        }
    }

    fn created_project_body() -> serde_json::Value {
        serde_json::json!({
            "project": {
                "id": "cold-sky-123", "name": "oxy-org-00000000-0000-0000-0000-000000000000",
                "region_id": "aws-us-east-2", "pg_version": 17,
                "proxy_host": "us-east-2.aws.neon.tech"
            },
            "branch": { "id": "br-main-1", "name": "main", "default": true },
            "databases": [{ "name": "neondb", "owner_name": "oxy_owner" }],
            "roles": [{ "name": "oxy_owner", "password": "disclosed-once", "branch_id": "br-main-1" }],
            "endpoints": [
                { "host": "ep-ro.neon.tech", "type": "read_only" },
                { "host": "ep-rw.neon.tech", "type": "read_write" }
            ],
            "operations": [{ "id": "op-1", "status": "running" }]
        })
    }

    async fn serve(state: StdArc<StubState>) -> String {
        let app = Router::new()
            .route(
                "/projects",
                routing::post(|State(s): State<StdArc<StubState>>| async move {
                    s.creates.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::CREATED, axum::Json(created_project_body()))
                })
                .get(|State(s): State<StdArc<StubState>>| async move {
                    let projects = if s.orphan_exists {
                        let name = s.orphan_name.clone().unwrap_or_else(|| {
                            "oxy-org-00000000-0000-0000-0000-000000000000".to_string()
                        });
                        serde_json::json!([{ "id": "cold-sky-123", "name": name }])
                    } else {
                        serde_json::json!([])
                    };
                    axum::Json(serde_json::json!({ "projects": projects }))
                }),
            )
            .route(
                "/projects/{pid}",
                routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "project": {
                            "id": "cold-sky-123", "name": "oxy-org-00000000-0000-0000-0000-000000000000",
                            "region_id": "aws-us-east-2", "pg_version": 17
                        }
                    }))
                }),
            )
            .route(
                "/projects/{pid}/branches",
                routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "branches": [{ "id": "br-main-1", "name": "main", "default": true }]
                    }))
                }),
            )
            .route(
                "/projects/{pid}/branches/{bid}/databases",
                routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "databases": [{ "name": "neondb", "owner_name": "oxy_owner" }]
                    }))
                }),
            )
            .route(
                "/projects/{pid}/endpoints",
                routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "endpoints": [{ "host": "ep-rw.neon.tech", "type": "read_write" }]
                    }))
                }),
            )
            .route(
                "/projects/{pid}/branches/{bid}/roles/{role}/reset_password",
                routing::post(|| async {
                    axum::Json(serde_json::json!({
                        "role": { "name": "oxy_owner", "password": "reissued" },
                        "operations": []
                    }))
                }),
            )
            .route(
                "/projects/{pid}/operations/{oid}",
                routing::get(|State(s): State<StdArc<StubState>>| async move {
                    let n = s.polls.fetch_add(1, Ordering::SeqCst);
                    let status = if s.fail_operation {
                        "failed"
                    } else if n >= s.finish_after {
                        "finished"
                    } else {
                        "running"
                    };
                    axum::Json(serde_json::json!({
                        "operation": { "id": "op-1", "status": status, "error": "compute never started" }
                    }))
                }),
            )
            .route(
                "/projects/missing",
                routing::get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/projects/limited",
                routing::get(|| async { StatusCode::TOO_MANY_REQUESTS }),
            )
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn client(base: &str) -> NeonProvider {
        NeonProvider::with_base_url("test-key", "org-oxy", base)
    }

    #[tokio::test]
    async fn create_project_waits_for_the_compute_before_returning() {
        let state = StdArc::new(StubState {
            finish_after: 2,
            ..Default::default()
        });
        let base = serve(state.clone()).await;

        let project = client(&base)
            .create_project(CreateProjectRequest {
                name: "oxy-org-00000000-0000-0000-0000-000000000000".into(),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect("create should succeed");

        assert!(
            state.polls.load(Ordering::SeqCst) >= 3,
            "client must poll until the operation finishes, polled {} time(s)",
            state.polls.load(Ordering::SeqCst)
        );
        // The read-write endpoint, not proxy_host and not the replica: a DSN
        // built from either of those resolves and then fails to authenticate.
        assert_eq!(project.host, "ep-rw.neon.tech");
        assert_eq!(project.branch.id, "br-main-1");
        assert_eq!(project.database.name, "neondb");
        assert_eq!(
            project.owner_role.password.as_deref(),
            Some("disclosed-once"),
            "the one moment the password is available must not be dropped"
        );
    }

    #[tokio::test]
    async fn a_failed_operation_fails_the_provision() {
        let state = StdArc::new(StubState {
            fail_operation: true,
            ..Default::default()
        });
        let base = serve(state).await;

        let err = client(&base)
            .create_project(CreateProjectRequest {
                name: "oxy-org-00000000-0000-0000-0000-000000000000".into(),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect_err("a failed operation must not read as a provisioned tenant");
        assert!(
            err.to_string().contains("compute never started"),
            "the provider's own reason should survive: {err}"
        );
    }

    #[tokio::test]
    async fn absent_is_none_and_rate_limited_is_retryable() {
        let base = serve(StdArc::new(StubState::default())).await;
        let c = client(&base);

        assert!(
            c.get_project("missing")
                .await
                .expect("404 is not an error")
                .is_none(),
            "a missing project is Ok(None), which is what makes delete idempotent"
        );

        let err = c.get_project("limited").await.expect_err("429");
        assert!(matches!(err, ProviderError::RateLimited));
        assert!(err.is_retryable(), "a rate limit is the retryable case");
    }

    /// A retry after a crash between "project created" and "row recorded" must
    /// adopt the orphan, not buy a second project.
    ///
    /// Neon does not enforce unique project names, so nothing server-side stops
    /// the duplicate — and an unattended provisioning loop would keep creating
    /// billable projects that nothing references.
    #[tokio::test]
    async fn a_retry_adopts_the_orphan_instead_of_creating_a_second_project() {
        let state = StdArc::new(StubState {
            orphan_exists: true,
            ..Default::default()
        });
        let base = serve(state.clone()).await;

        // A real derived name. `oxy-org-acme` was a placeholder that only
        // worked while adoption was ungated — production names come from
        // `project_name_for`, which is `oxy-org-<uuid>`, and that is now what
        // the adoption path requires.
        let project = client(&base)
            .create_project(CreateProjectRequest {
                name: crate::provisioner::project_name_for(uuid::Uuid::nil()),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect("the orphan should be adopted");

        assert_eq!(
            state.creates.load(Ordering::SeqCst),
            0,
            "an existing project must never be duplicated"
        );
        assert_eq!(project.id, "cold-sky-123");
        assert_eq!(
            project.owner_role.password.as_deref(),
            Some("reissued"),
            "the lost password is unrecoverable, so adoption must reset it"
        );
    }

    /// A chosen name that ALREADY EXISTS must be refused, not adopted and not
    /// duplicated.
    ///
    /// Three answers were possible and two of them were wrong. Adoption resets
    /// the found project's owner password and takes it over, which is only safe
    /// for a name identifying one org — the `airhouse` incident was user-chosen
    /// names, where that crosses tenants. Creating anyway is worse still:
    /// Neon does not enforce unique project names, so it silently produces a
    /// second billable project.
    ///
    /// The stub's orphan carries THIS test's name on purpose. Reporting a
    /// derived orphan instead meant `find_project_id_by_name("bookings")`
    /// matched nothing, returned `None`, and the guard was never entered — so
    /// the previous version of this test asserted the right thing about the
    /// wrong state, and deleting the guard left it green.
    #[tokio::test]
    async fn a_chosen_name_that_exists_is_refused_not_adopted_or_duplicated() {
        let state = StdArc::new(StubState::orphan_named("bookings"));
        let base = serve(state.clone()).await;

        let err = client(&base)
            .create_project(CreateProjectRequest {
                name: "bookings".into(),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect_err("a chosen name that already exists must be refused");

        assert!(
            matches!(err, ProviderError::ProjectNameTaken(ref n) if n == "bookings"),
            "the refusal must be the collision error naming the project: {err:?}"
        );
        assert_eq!(
            state.creates.load(Ordering::SeqCst),
            0,
            "refusing must not also create — Neon allows duplicate names, so a \
             fall-through here is a second billable project"
        );
    }

    /// A chosen name with NO collision still creates, as it always did.
    ///
    /// The companion to the case above: derivation gates the ANSWER when
    /// something is found, not whether the lookup runs.
    #[tokio::test]
    async fn a_chosen_name_with_no_collision_still_creates() {
        let state = StdArc::new(StubState::default());
        let base = serve(state.clone()).await;

        client(&base)
            .create_project(CreateProjectRequest {
                name: "bookings".into(),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect("a chosen name with nothing in the way creates");

        assert_eq!(state.creates.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn with_no_orphan_it_creates_exactly_one_project() {
        let state = StdArc::new(StubState::default());
        let base = serve(state.clone()).await;

        client(&base)
            .create_project(CreateProjectRequest {
                name: "oxy-org-00000000-0000-0000-0000-000000000000".into(),
                region_id: "aws-us-east-2".into(),
                pg_version: 17,
            })
            .await
            .expect("create");

        assert_eq!(state.creates.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_collision_is_recognised_only_on_the_statuses_that_mean_it() {
        assert!(is_name_taken(409, "project with name already exists"));
        assert!(is_name_taken(422, "The name is already in use"));
        // A 500 whose body happens to contain the phrase must not be mistaken
        // for a collision — retrying is right for one and wrong for the other.
        assert!(!is_name_taken(500, "already exists"));
        assert!(!is_name_taken(409, "quota exceeded"));
    }

    #[test]
    fn the_read_write_endpoint_wins_over_a_replica() {
        let eps = vec![
            WireEndpoint {
                host: "ro.neon.tech".into(),
                r#type: Some("read_only".into()),
            },
            WireEndpoint {
                host: "rw.neon.tech".into(),
                r#type: Some("read_write".into()),
            },
        ];
        assert_eq!(read_write_host(&eps).as_deref(), Some("rw.neon.tech"));
        assert_eq!(read_write_host(&[]), None);
    }

    #[test]
    fn error_bodies_degrade_to_something_an_operator_can_read() {
        assert_eq!(error_message(r#"{"message":"nope","code":"x"}"#), "nope");
        assert_eq!(error_message(""), "no response body");
        assert_eq!(error_message("<html>502</html>"), "<html>502</html>");
    }

    #[test]
    fn a_password_is_never_carried_out_of_a_read() {
        let raw = serde_json::json!({ "role": { "name": "app_x_rw", "password": "leaked" } });
        let role = parse_role(&raw).unwrap().redacted();
        assert!(role.password.is_none());
    }

    #[test]
    fn retryability_matches_what_a_caller_should_do() {
        assert!(ProviderError::RateLimited.is_retryable());
        assert!(
            ProviderError::Api {
                status: 503,
                message: "upstream".into()
            }
            .is_retryable()
        );
        assert!(
            !ProviderError::ProjectNameTaken("oxy-org-00000000-0000-0000-0000-000000000000".into())
                .is_retryable(),
            "a name collision never resolves by retrying"
        );
    }
}
