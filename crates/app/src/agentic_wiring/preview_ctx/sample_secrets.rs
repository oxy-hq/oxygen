//! A sample platform's secrets: what may resolve, and the one write-back.
//!
//! * [`PreviewPlatformContext::secret_withheld`] — production's QuickBooks
//!   vars never resolve on any preview platform; on a rotate-on-use sample's
//!   platform nothing outside the registered sandbox's var names does either.
//! * [`PreviewPlatformContext::persist_sandbox_token`] — the sandbox's rotating
//!   var, **updated** in the workspace's secrets table. An absent secret is an
//!   error, never created: the grant's token is stored by staff, and a sample
//!   that could create secrets could mint any name it was handed.

use oxy::service::secret_manager::UpdateSecretParams;
use oxy_shared::errors::OxyError;

use super::PreviewPlatformContext;
use super::sample::SampleSide;
use crate::server::service::secret_manager::SecretManagerService;

/// Why a write of `var` was refused.
fn refused(var: &str) -> String {
    format!(
        "`{var}` is not written: a workspace preview persists no secrets but its sample's own \
         sandbox token"
    )
}

impl PreviewPlatformContext {
    /// `var` must not resolve here (module doc).
    pub(super) fn secret_withheld(&self, var: &str) -> bool {
        if self.withheld_secrets.contains(var) {
            return true;
        }
        self.sample.as_ref().and_then(|s| s.allows(var)) == Some(false)
    }

    /// Whether `var` is the sandbox grant this sample rotates — the one secret
    /// a preview may write — and not one of production's.
    pub(super) fn may_persist(&self, var: &str) -> bool {
        let rotating = self
            .sample
            .as_ref()
            .and_then(SampleSide::sandbox)
            .and_then(|s| s.rotating_var());
        rotating == Some(var) && !self.secret_withheld(var)
    }

    /// The rotated token, written back in place (module doc).
    pub(super) async fn persist_sandbox_token(&self, var: &str, value: &str) -> Result<(), String> {
        if !self.may_persist(var) {
            return Err(refused(var));
        }
        let store = self
            .sample
            .as_ref()
            .and_then(|s| s.store.as_ref())
            .ok_or_else(|| format!("`{var}` is not written: this platform has no secret store"))?;
        let params = UpdateSecretParams {
            value: Some(value.to_string()),
            description: None,
            updated_by: store.updated_by,
        };
        match SecretManagerService::new(self.workspace_id)
            .update_secret(&store.db, var, params)
            .await
        {
            Ok(_) => Ok(()),
            Err(OxyError::SecretManager(_)) => Err(format!(
                "`{var}` is not a stored workspace secret: a sample rotates its sandbox grant in \
                 place and never creates one — store the sandbox's refresh token first"
            )),
            Err(e) => Err(format!("persisting `{var}`: {e}")),
        }
    }
}
