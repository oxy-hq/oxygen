//! In-memory provider shaped like Neon. **Provisions nothing real.**
//!
//! Deterministic by construction: ids and passwords come from a counter, not a
//! clock or an RNG, so tests assert on exact values and stay reproducible.
//!
//! Supports fault injection ([`MockProvider::push_fault`]) so the provisioner's
//! failure semantics — partial provision, retry, reconcile — are testable
//! without a live provider that can be made to fail on demand.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;

use super::types::{
    Branch, BranchRequest, CreateProjectRequest, DatabaseInfo, Project, ProjectBranch, Role,
};
use super::{OltpProvider, ProviderError};

/// Key for a role within a project branch.
type RoleKey = (String, String, String);

#[derive(Default)]
struct State {
    /// project id → project
    projects: HashMap<String, Project>,
    /// project name → project id, enforcing the provider's uniqueness rule
    names_taken: HashMap<String, String>,
    /// (project, branch, role) → current password
    roles: HashMap<RoleKey, String>,
    /// (project, branch id) → a non-default branch, stored redacted
    branches: HashMap<(String, String), ProjectBranch>,
    /// branch id → how many times it was reset, for assertions
    branch_resets: HashMap<String, u32>,
    /// Every branch id `delete_branch` was asked to delete, refused or not —
    /// so a test can prove a caller never ASKED to delete production, which a
    /// refusal here would otherwise hide.
    branch_delete_attempts: Vec<String>,
    /// Held before a branch create answers, so a test can make two provisions
    /// overlap deterministically.
    branch_create_delay: Option<std::time::Duration>,
    seq: u64,
    faults: VecDeque<ProviderError>,
}

impl State {
    fn next(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Pop an injected fault, if the test queued one for this call.
    fn take_fault(&mut self) -> Option<ProviderError> {
        self.faults.pop_front()
    }
}

#[derive(Default)]
pub struct MockProvider {
    state: Mutex<State>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a failure for the next provider call, whatever it is. Calls pop
    /// faults in FIFO order, so a test can script a sequence.
    pub fn push_fault(&self, err: ProviderError) {
        self.state.lock().expect("mock lock").faults.push_back(err);
    }

    pub fn project_count(&self) -> usize {
        self.state.lock().expect("mock lock").projects.len()
    }

    /// Role names on a branch, sorted — for assertions.
    pub fn role_names(&self, project_id: &str, branch_id: &str) -> Vec<String> {
        let state = self.state.lock().expect("mock lock");
        let mut names: Vec<String> = state
            .roles
            .keys()
            .filter(|(p, b, _)| p == project_id && b == branch_id)
            .map(|(_, _, r)| r.clone())
            .collect();
        names.sort();
        names
    }

    /// Non-default branches across every project.
    pub fn branch_count(&self) -> usize {
        self.state.lock().expect("mock lock").branches.len()
    }

    /// How many times `branch_id` has been reset.
    pub fn branch_resets(&self, branch_id: &str) -> u32 {
        self.state
            .lock()
            .expect("mock lock")
            .branch_resets
            .get(branch_id)
            .copied()
            .unwrap_or(0)
    }

    /// Current password for a role, if it exists. Test-only: a real provider
    /// never re-discloses this.
    pub fn peek_password(&self, project_id: &str, branch_id: &str, role: &str) -> Option<String> {
        let key = (
            project_id.to_string(),
            branch_id.to_string(),
            role.to_string(),
        );
        self.state
            .lock()
            .expect("mock lock")
            .roles
            .get(&key)
            .cloned()
    }
}

#[async_trait]
impl OltpProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn create_project(&self, req: CreateProjectRequest) -> Result<Project, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        if state.names_taken.contains_key(&req.name) {
            return Err(ProviderError::ProjectNameTaken(req.name));
        }

        let n = state.next();
        let owner_name = super::OWNER_ROLE.to_string();
        let project = Project {
            id: format!("proj-{n}"),
            name: req.name.clone(),
            region_id: req.region_id,
            pg_version: req.pg_version,
            branch: Branch {
                id: format!("br-{n}"),
                name: "main".to_string(),
            },
            database: DatabaseInfo {
                name: "neondb".to_string(),
                owner_name: owner_name.clone(),
            },
            owner_role: Role {
                name: owner_name.clone(),
                password: Some(format!("mock-pw-{n}")),
            },
            host: format!("ep-{n}.mock.local"),
        };

        state.roles.insert(
            (project.id.clone(), project.branch.id.clone(), owner_name),
            format!("mock-pw-{n}"),
        );
        state.names_taken.insert(req.name, project.id.clone());
        // Stored redacted: re-reads must not re-disclose the owner password,
        // matching the real provider.
        let mut stored = project.clone();
        stored.owner_role = stored.owner_role.redacted();
        state.projects.insert(project.id.clone(), stored);

        Ok(project)
    }

    async fn get_project(&self, project_id: &str) -> Result<Option<Project>, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        Ok(state.projects.get(project_id).cloned())
    }

    async fn delete_project(&self, project_id: &str) -> Result<(), ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        if let Some(p) = state.projects.remove(project_id) {
            state.names_taken.remove(&p.name);
            state.roles.retain(|(proj, _, _), _| proj != project_id);
            // A project's branches die with it, as on Neon.
            state.branches.retain(|(proj, _), _| proj != project_id);
        }
        // Idempotent: absent is success.
        Ok(())
    }

    async fn create_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        if !state.projects.contains_key(project_id) {
            return Err(ProviderError::ProjectNotFound(project_id.to_string()));
        }
        let n = state.next();
        let password = format!("mock-pw-{n}");
        state.roles.insert(
            (
                project_id.to_string(),
                branch_id.to_string(),
                role_name.to_string(),
            ),
            password.clone(),
        );
        Ok(Role {
            name: role_name.to_string(),
            password: Some(password),
        })
    }

    async fn get_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Option<Role>, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        let key = (
            project_id.to_string(),
            branch_id.to_string(),
            role_name.to_string(),
        );
        // Password deliberately withheld: the real provider only ever
        // discloses it at create/reset.
        Ok(state.roles.get(&key).map(|_| Role {
            name: role_name.to_string(),
            password: None,
        }))
    }

    async fn reset_role_password(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<Role, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        let key = (
            project_id.to_string(),
            branch_id.to_string(),
            role_name.to_string(),
        );
        if !state.roles.contains_key(&key) {
            return Err(ProviderError::RoleNotFound(
                role_name.to_string(),
                branch_id.to_string(),
            ));
        }
        let n = state.next();
        let password = format!("mock-pw-{n}");
        state.roles.insert(key, password.clone());
        Ok(Role {
            name: role_name.to_string(),
            password: Some(password),
        })
    }

    async fn delete_role(
        &self,
        project_id: &str,
        branch_id: &str,
        role_name: &str,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        state.roles.remove(&(
            project_id.to_string(),
            branch_id.to_string(),
            role_name.to_string(),
        ));
        Ok(())
    }

    async fn create_branch(&self, req: &BranchRequest) -> Result<ProjectBranch, ProviderError> {
        // Read, then release the lock before sleeping: a std mutex held across
        // an await would serialize exactly the overlap a test is asking for.
        let delay = self.state.lock().expect("mock lock").branch_create_delay;
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        self.create_branch_impl(req)
    }

    async fn reset_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        self.reset_branch_impl(req, branch_id)
    }

    async fn delete_branch(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        self.delete_branch_impl(req, branch_id)
    }

    /// As on Neon, a project's branches go with it.
    fn project_delete_takes_branches(&self) -> bool {
        true
    }
}

// Branches, in their own file to keep this one readable.
mod branches;

#[cfg(test)]
mod branch_tests;

#[cfg(test)]
mod tests;
