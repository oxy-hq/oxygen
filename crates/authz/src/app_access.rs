//! An app's own access to the platform: the actions an org admin approved for
//! one custom app in one org (`internal-docs/proposals/app-platform-access.md`).
//!
//! A function of the app asks for one of them through a host op. The host
//! decides that call as the app's **service account**, whose credential carries
//! an [`AppReach`] on its [`TokenReach`](crate::TokenReach). Like a sandbox
//! agent token, the reach is narrower than any grant can say: [`crate::allows`]
//! asks it first, and it can only refuse. What it covers is then decided by the
//! ring for the account, which stands as an admin of its one org.
//!
//! The vocabulary is closed. Only the actions in [`Action::APP_ACTIONS`] can
//! be approved for an app; each is an action the platform chose to offer
//! apps, with a host op of its own.

use uuid::Uuid;

use crate::{Action, Resource, ResourceKind};

/// One approved action, in one org. Approvals are the org's and the app's: no
/// narrower unit. What an operation acts on inside the org (a kiosk, a
/// pipeline's landing zone) is checked by the host op that performs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppActionGrant {
    pub action: Action,
    pub org_id: Uuid,
}

/// What an app's credential reaches: its approved actions and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppReach {
    pub app_id: Uuid,
    pub grants: Vec<AppActionGrant>,
}

impl AppReach {
    /// Does an approval cover this decision? `false` ends it; `true` hands it to
    /// the ring. The action must be approved, in the resource's org, on the org
    /// or on something of the org a ring decides an action on (a workspace, for
    /// the upload route's `WorkspaceEditor` ring). Never the platform, a
    /// partner, an app's admin surface or a thread.
    pub(crate) fn covers(&self, action: Action, resource: &Resource) -> bool {
        matches!(resource.kind, ResourceKind::Org | ResourceKind::Workspace)
            && self
                .grants
                .iter()
                .any(|grant| grant.action == action && grant.org_id == resource.org_id)
    }
}

impl Action {
    /// Every action an org admin may approve for an app. A new entry offers
    /// app code something new: it needs a host op that decides on it, and an
    /// arm in [`Self::app_action_id`].
    pub const APP_ACTIONS: [Action; 2] =
        [Action::FrontlineDeviceManage, Action::SourceUploadReport];

    /// The id an approval is stored and shown under, for an action apps may be
    /// approved for. `None` for every other action, so nothing outside the
    /// list can be written as an approval or read back as one.
    pub fn app_action_id(self) -> Option<&'static str> {
        Action::APP_ACTIONS.contains(&self).then(|| self.as_str())
    }

    /// The app action stored under `id`. `None` for an unknown id and for any
    /// action that is not an app action, so a row naming one grants nothing.
    pub fn from_app_action_id(id: &str) -> Option<Action> {
        Action::APP_ACTIONS
            .into_iter()
            .find(|action| action.as_str() == id)
    }

    /// Whether the ring that decides the action takes a workspace resource
    /// (the upload route's `WorkspaceEditor`) rather than the org. Where an
    /// operation needs one, the host op resolves it; an approval never names one.
    pub fn app_action_on_workspace(self) -> bool {
        matches!(self, Action::SourceUploadReport)
    }
}

#[cfg(test)]
#[path = "app_access_tests.rs"]
mod tests;
