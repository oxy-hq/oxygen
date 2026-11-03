//! The mock's branches, shaped like Neon's: each has its own id, endpoint and
//! owner password; a branch already carrying the name is adopted (in the shape
//! asked for only, as on Neon); a reset keeps
//! the id; the project's default branch cannot be deleted.

use super::{MockProvider, State};
use crate::provider::ProviderError;
use crate::provider::types::{BranchInit, BranchRequest, DatabaseInfo, ProjectBranch, Role};

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

    /// Every request `create_branch` received, in order — failed ones included,
    /// since what the caller asked for is the fact, not whether it landed.
    pub fn branch_create_requests(&self) -> Vec<BranchRequest> {
        self.state
            .lock()
            .expect("mock lock")
            .branch_create_requests
            .clone()
    }

    /// Make every `create_branch` wait this long before answering — the width
    /// of the race a concurrency test needs two provisions to share.
    pub fn delay_branch_creates(&self, delay: std::time::Duration) {
        self.state.lock().expect("mock lock").branch_create_delay = Some(delay);
    }

    /// Every `(branch id, expires_at)` `set_branch_expiry` was asked for, in
    /// order.
    pub fn branch_expiry_updates(&self) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
        self.state
            .lock()
            .expect("mock lock")
            .branch_expiry_updates
            .clone()
    }

    /// The init a live branch was created with — `SchemaOnly` is a branch
    /// that holds none of its parent's rows. `None` once it is deleted.
    pub fn branch_init(&self, branch_id: &str) -> Option<BranchInit> {
        self.state
            .lock()
            .expect("mock lock")
            .branch_inits
            .get(branch_id)
            .copied()
    }

    pub(super) fn find_branch_id_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<Option<String>, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        Ok(state
            .branches
            .iter()
            .find(|((p, _), b)| p == &req.project_id && b.name == req.name)
            .map(|((_, id), _)| id.clone()))
    }

    pub(super) fn set_branch_expiry_impl(
        &self,
        req: &BranchRequest,
        branch_id: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        if let Some(f) = state.take_fault() {
            return Err(f);
        }
        let key = (req.project_id.clone(), branch_id.to_string());
        if !state.branches.contains_key(&key) {
            return Err(ProviderError::BranchNotFound(branch_id.to_string()));
        }
        state
            .branch_expiry_updates
            .push((branch_id.to_string(), expires_at));
        Ok(())
    }

    pub(super) fn create_branch_impl(
        &self,
        req: &BranchRequest,
    ) -> Result<ProjectBranch, ProviderError> {
        let mut state = self.state.lock().expect("mock lock");
        state.branch_create_requests.push(req.clone());
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
        if let Some(found) = &adopted {
            refuse_other_shape(&state, req, found)?;
        }
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
        state.branch_inits.insert(branch.id.clone(), req.init);
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
        state.branch_inits.remove(branch_id);
        state
            .roles
            .retain(|(p, b, _), _| !(p == project_id && b == branch_id));
        Ok(())
    }
}

/// Neon's adopt rule: a same-named branch is adopted only in the shape asked
/// for. A data branch has production as its parent and a schema-only one has
/// none, so the other shape answers as a parent mismatch.
fn refuse_other_shape(
    state: &State,
    req: &BranchRequest,
    found: &ProjectBranch,
) -> Result<(), ProviderError> {
    let init = state
        .branch_inits
        .get(&found.id)
        .copied()
        .unwrap_or_default();
    if init == req.init {
        return Ok(());
    }
    let parent = match init {
        BranchInit::ParentData => found.parent_id.clone(),
        BranchInit::SchemaOnly => String::new(),
    };
    Err(ProviderError::BranchParentMismatch {
        name: req.name.clone(),
        parent,
        expected: req.parent_branch_id.clone(),
    })
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
