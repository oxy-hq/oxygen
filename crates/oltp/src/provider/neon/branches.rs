//! Neon branches — the org's staging copy of its tenant database.
//!
//! A Neon branch is a copy-on-write fork of the whole Postgres: every schema,
//! row and role, with the roles' password hashes, on a compute endpoint of its
//! own. So a branch is reached exactly like production except for the host —
//! and, once the owner's password is reset on it, never with production's
//! owner credential.
//!
//! Wire contract from Neon's published OpenAPI spec
//! (<https://neon.tech/api_spec/release/v2.json>):
//!
//! | call | endpoint |
//! | --- | --- |
//! | create | `POST /projects/{p}/branches` `{branch:{name,parent_id}, endpoints:[{type:"read_write"}]}` |
//! | find by name | `GET /projects/{p}/branches` |
//! | endpoint | `GET /projects/{p}/branches/{b}/endpoints`, else `POST /projects/{p}/endpoints` |
//! | reset | `POST /projects/{p}/branches/{b}/restore` `{source_branch_id: <parent>}` — Neon's "reset from parent" |
//! | delete | `GET /projects/{p}/branches/{b}` (must be a non-default, unprotected child of production), then `DELETE` it |
//!
//! **Only ever exercised against a stub** speaking those shapes (`tests.rs`
//! beside this file); no test here calls Neon. Every write returns
//! `operations[]` and is awaited like a project create, for the same reason:
//! the endpoint refuses connections until they finish.

use serde::Deserialize;
use tracing::warn;

use super::{NeonProvider, WireBranch, WireEndpoint};
use crate::provider::ProviderError;
use crate::provider::types::{BranchRequest, DatabaseInfo, ProjectBranch};

#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize)]
struct CreatedBranch {
    branch: WireBranch,
    #[serde(default)]
    endpoints: Vec<WireEndpoint>,
}

impl NeonProvider {
    pub(super) async fn create_branch_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<ProjectBranch, ProviderError> {
        let (id, host) = match self.find_branch(&req.project_id, &req.name).await? {
            // Adopt: the name is derived from the branch kind inside one org's
            // project, so a match is an earlier attempt's branch that crashed
            // before it was recorded. Neon does not refuse a duplicate branch
            // name reliably enough to lean on, and a second branch is a second
            // billable compute.
            Some(found) => {
                let id = found.id.clone();
                refuse_production(req, &id)?;
                // Only OUR half-finished branch is adoptable: one cut from
                // production. A same-named branch cut from anywhere else would
                // serve staging from someone else's data.
                if found.parent_id.as_deref() != Some(req.parent_branch_id.as_str()) {
                    return Err(ProviderError::BranchParentMismatch {
                        name: req.name.clone(),
                        parent: found.parent_id.unwrap_or_default(),
                        expected: req.parent_branch_id.clone(),
                    });
                }
                warn!(
                    branch_id = %id,
                    name = %req.name,
                    "adopting an existing Neon branch — a previous provision created it \
                     but did not record it locally"
                );
                let host = self.read_write_endpoint(&req.project_id, &id).await?;
                (id, host)
            }
            None => self.post_branch(req).await?,
        };
        self.with_fresh_owner(req, id, host).await
    }

    pub(super) async fn reset_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        // Before any request: restoring production "from its parent" is not a
        // reset of anything, and nothing on Neon's side stops the call.
        refuse_production(req, branch_id)?;
        let path = format!("/projects/{}/branches/{branch_id}/restore", req.project_id);
        let body = serde_json::json!({ "source_branch_id": req.parent_branch_id });
        let raw = self
            .send(reqwest::Method::POST, &path, Some(body))
            .await?
            .ok_or_else(|| ProviderError::BranchNotFound(branch_id.to_string()))?;
        self.await_operations(&req.project_id, &raw).await?;
        let host = self.read_write_endpoint(&req.project_id, branch_id).await?;
        // The restore brought production's role table with it, owner included,
        // so production's owner password opens the branch again until this runs.
        self.with_fresh_owner(req, branch_id.to_string(), host)
            .await
    }

    pub(super) async fn delete_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        refuse_production(req, branch_id)?;
        let path = format!("/projects/{}/branches/{branch_id}", req.project_id);
        // Look before deleting, and refuse on what Neon says the branch IS —
        // not only on the id Oxy recorded. A row pointing at the project's
        // default, primary or protected branch is a corrupted record, and
        // Neon's own refusal to delete a default branch is not a guard this
        // code should be leaning on for production. `None` is a 404: already
        // gone, which the trait defines as success.
        let Some(described) = self.send(reqwest::Method::GET, &path, None).await? else {
            return Ok(());
        };
        described_as_deletable(req, branch_id, described)?;
        if let Some(raw) = self.send(reqwest::Method::DELETE, &path, None).await? {
            self.await_operations(&req.project_id, &raw).await?;
        }
        Ok(())
    }

    async fn post_branch(&self, req: &BranchRequest) -> Result<(String, String), ProviderError> {
        let path = format!("/projects/{}/branches", req.project_id);
        let body = serde_json::json!({
            "branch": { "name": req.name, "parent_id": req.parent_branch_id },
            "endpoints": [{ "type": "read_write" }],
        });
        let raw = self
            .send(reqwest::Method::POST, &path, Some(body))
            .await?
            .ok_or_else(|| ProviderError::ProjectNotFound(req.project_id.clone()))?;
        let created: CreatedBranch =
            serde_json::from_value(raw.clone()).map_err(|e| ProviderError::Api {
                status: 200,
                message: format!("unexpected create-branch response: {e}"),
            })?;
        self.await_operations(&req.project_id, &raw).await?;
        let host = strict_read_write(&created.endpoints).ok_or_else(|| ProviderError::Api {
            status: 200,
            message: "create-branch returned no read-write endpoint".into(),
        })?;
        Ok((created.branch.id, host))
    }

    /// Reset the owner's password ON THE BRANCH and describe it.
    ///
    /// This is what makes the branch's credential its own: the copy carries
    /// production's password hash, and until this runs the production owner
    /// password opens staging.
    async fn with_fresh_owner(
        &self,
        req: &BranchRequest,
        id: String,
        host: String,
    ) -> Result<ProjectBranch, ProviderError> {
        use crate::provider::OltpProvider;
        let owner_role = self
            .reset_role_password(&req.project_id, &id, &req.owner_role)
            .await?;
        Ok(ProjectBranch {
            id,
            name: req.name.clone(),
            parent_id: req.parent_branch_id.clone(),
            host,
            database: DatabaseInfo {
                name: req.database_name.clone(),
                owner_name: req.owner_role.clone(),
            },
            owner_role,
        })
    }

    /// The branch named exactly `name`, if the project has one.
    async fn find_branch(
        &self,
        project_id: &str,
        name: &str,
    ) -> Result<Option<WireBranch>, ProviderError> {
        let path = format!("/projects/{project_id}/branches");
        let body = self
            .send(reqwest::Method::GET, &path, None)
            .await?
            .ok_or_else(|| ProviderError::ProjectNotFound(project_id.to_string()))?;
        let Some(found) = body
            .get("branches")
            .and_then(|b| b.as_array())
            .and_then(|bs| {
                bs.iter()
                    .find(|b| b.get("name").and_then(|n| n.as_str()) == Some(name))
            })
        else {
            return Ok(None);
        };
        serde_json::from_value(found.clone())
            .map(Some)
            .map_err(|e| ProviderError::Api {
                status: 200,
                message: format!("unexpected branch shape: {e}"),
            })
    }

    /// The branch's read-write host, creating the endpoint if it has none.
    ///
    /// An adopted branch may have lost its compute (or never got one, if the
    /// earlier attempt died between the two), and a read-only replica is not a
    /// place staging can write — so "some endpoint" is not good enough here,
    /// unlike [`super::read_write_host`]'s fallback for a project.
    async fn read_write_endpoint(
        &self,
        project_id: &str,
        branch_id: &str,
    ) -> Result<String, ProviderError> {
        let path = format!("/projects/{project_id}/branches/{branch_id}/endpoints");
        let listed = self
            .send(reqwest::Method::GET, &path, None)
            .await?
            .ok_or_else(|| ProviderError::BranchNotFound(branch_id.to_string()))?;
        let endpoints: Vec<WireEndpoint> = listed
            .get("endpoints")
            .and_then(|e| serde_json::from_value(e.clone()).ok())
            .unwrap_or_default();
        if let Some(host) = strict_read_write(&endpoints) {
            return Ok(host);
        }
        let body = serde_json::json!({
            "endpoint": { "branch_id": branch_id, "type": "read_write" }
        });
        let raw = self
            .send(
                reqwest::Method::POST,
                &format!("/projects/{project_id}/endpoints"),
                Some(body),
            )
            .await?
            .ok_or_else(|| ProviderError::ProjectNotFound(project_id.to_string()))?;
        self.await_operations(project_id, &raw).await?;
        raw.get("endpoint")
            .and_then(|e| e.get("host"))
            .and_then(|h| h.as_str())
            .map(String::from)
            .ok_or_else(|| ProviderError::Api {
                status: 200,
                message: "create-endpoint returned no host".into(),
            })
    }
}

fn strict_read_write(endpoints: &[WireEndpoint]) -> Option<String> {
    endpoints
        .iter()
        .find(|e| e.r#type.as_deref() == Some("read_write"))
        .map(|e| e.host.clone())
}

/// Neon's description of a branch, as far as a delete depends on it.
/// `default` and `protected` are required: a body without them proves
/// nothing, and a delete that cannot prove its target is not production does
/// not run.
#[derive(Debug, Deserialize)]
struct DescribedBranch {
    id: String,
    #[serde(default)]
    parent_id: Option<String>,
    default: bool,
    protected: bool,
    /// Deprecated by Neon in favour of `default`; honoured when present.
    #[serde(default)]
    primary: bool,
}

/// Fail closed on `GET …/branches/{b}`: the branch must be the one asked for,
/// neither the project's default, primary nor a protected branch, and cut
/// from `req`'s parent — production. Anything else, including a body in a
/// shape this does not recognise, refuses the delete.
fn described_as_deletable(
    req: &BranchRequest,
    branch_id: &str,
    body: serde_json::Value,
) -> Result<(), ProviderError> {
    let unproven = |why: String| ProviderError::Api {
        status: 200,
        message: format!("refusing to delete branch {branch_id:?}: {why}"),
    };
    let raw = body
        .get("branch")
        .cloned()
        .ok_or_else(|| unproven("Neon's description of it has no `branch`".into()))?;
    let described: DescribedBranch = serde_json::from_value(raw)
        .map_err(|e| unproven(format!("unexpected branch shape: {e}")))?;
    if described.id != branch_id {
        return Err(unproven(format!(
            "Neon described {:?} instead",
            described.id
        )));
    }
    if described.default || described.primary || described.protected {
        return Err(ProviderError::BranchIsProduction(branch_id.to_string()));
    }
    if described.parent_id.as_deref() != Some(req.parent_branch_id.as_str()) {
        return Err(ProviderError::BranchParentMismatch {
            name: branch_id.to_string(),
            parent: described.parent_id.unwrap_or_default(),
            expected: req.parent_branch_id.clone(),
        });
    }
    Ok(())
}

/// A branch operation must never land on the branch it was cut from.
fn refuse_production(req: &BranchRequest, branch_id: &str) -> Result<(), ProviderError> {
    if branch_id == req.parent_branch_id {
        return Err(ProviderError::BranchIsProduction(branch_id.to_string()));
    }
    Ok(())
}
