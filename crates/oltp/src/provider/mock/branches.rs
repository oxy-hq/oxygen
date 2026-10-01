//! The mock's branches, shaped like Neon's: each has its own id, endpoint and
//! owner password; a branch already carrying the name is adopted; a reset keeps
//! the id; the project's default branch cannot be deleted.

use super::{MockProvider, State};
use crate::provider::ProviderError;
use crate::provider::types::{BranchRequest, DatabaseInfo, ProjectBranch, Role};

impl MockProvider {
    /// Every branch id `delete_branch` was asked to delete, in order —
    /// including ones it refused.
    pub fn branch_delete_attempts(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("mock lock")
            .branch_delete_attempts
            .clone()
    }

    /// Make every `create_branch` wait this long before answering — the width
    /// of the race a concurrency test needs two provisions to share.
    pub fn delay_branch_creates(&self, delay: std::time::Duration) {
        self.state.lock().expect("mock lock").branch_create_delay = Some(delay);
    }

    pub(super) fn create_branch_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<ProjectBranch, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        if !state.projects.contains_key(&req.project_id) {
            return Err(ProviderError::ProjectNotFound(req.project_id.clone()));
        }
        let adopted = state
            .branches
            .iter()
            .find(|((p, _), b)| p == &req.project_id && b.name == req.name)
            .map(|(_, b)| b.clone());
        let n = state.next();
        let branch = adopted.unwrap_or_else(|| ProjectBranch {
            id: format!("br-{n}"),
            name: req.name.clone(),
            parent_id: req.parent_branch_id.clone(),
            host: format!("ep-{n}.mock.local"),
            database: DatabaseInfo {
                name: req.database_name.clone(),
                owner_name: req.owner_role.clone(),
            },
            owner_role: Role {
                name: req.owner_role.clone(),
                password: None,
            },
        });
        state
            .branches
            .insert((req.project_id.clone(), branch.id.clone()), branch.clone());
        Ok(disclose_owner(&mut state, &req.project_id, branch, n))
    }

    pub(super) fn reset_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<ProjectBranch, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        let key = (req.project_id.clone(), branch_id.to_string());
        let Some(branch) = state.branches.get(&key).cloned() else {
            return Err(ProviderError::BranchNotFound(branch_id.to_string()));
        };
        *state
            .branch_resets
            .entry(branch_id.to_string())
            .or_default() += 1;
        let n = state.next();
        Ok(disclose_owner(&mut state, &req.project_id, branch, n))
    }

    pub(super) fn delete_branch_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        // Recorded before anything can refuse: the attempt is the fact a test
        // about the CALLER's guard needs, and Neon-shaped refusal below would
        // otherwise make a missing guard indistinguishable from a working one.
        state.branch_delete_attempts.push(branch_id.to_string());
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        let project_id = req.project_id.as_str();
        if state
            .projects
            .get(project_id)
            .is_some_and(|p| p.branch.id == branch_id)
        {
            return Err(ProviderError::BranchIsProduction(branch_id.to_string()));
        }
        state
            .branches
            .remove(&(project_id.to_string(), branch_id.to_string()));
        state
            .roles
            .retain(|(p, b, _), _| !(p == project_id && b == branch_id));
        Ok(())
    }
}

/// Mint a fresh owner password on `branch`, record it, and disclose it once —
/// the part `create_branch` and `reset_branch` share.
fn disclose_owner(
    state: &mut State,
    project_id: &str,
    mut branch: ProjectBranch,
    n: u64,
) -> ProjectBranch {
    let password = format!("mock-branch-pw-{n}");
    state.roles.insert(
        (
            project_id.to_string(),
            branch.id.clone(),
            branch.owner_role.name.clone(),
        ),
        password.clone(),
    );
    branch.owner_role.password = Some(password);
    branch
}
