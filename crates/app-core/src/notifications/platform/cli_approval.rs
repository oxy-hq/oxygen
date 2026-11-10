//! `oxy.cli_approval`, the write: tell the approver a device-code request
//! waits on them. Called by the start of the device-code flow
//! (`user_tokens/device_login.rs`); the words are the notifications surface's
//! (`platform::cli_approval` in `oxy-api-notifications`).

use sea_orm::{DatabaseConnection, DbErr};
use uuid::Uuid;

use super::kinds::PlatformNotice;
use super::people;
use super::send::{PlatformSent, notify};

/// Tell `user_id` that the CLI request `user_code` (stored form, no dash)
/// waits on them, filed under their oldest org. `None` when they belong to no
/// org: there is nowhere to file the notice.
pub async fn notify_cli_approval(
    db: &DatabaseConnection,
    user_id: Uuid,
    user_code: &str,
) -> Result<Option<PlatformSent>, DbErr> {
    let Some(org_id) = people::orgs_of(db, user_id).await?.into_iter().next() else {
        return Ok(None);
    };
    let notice = PlatformNotice::CliApproval {
        org_id,
        user_code: user_code.to_string(),
    };
    notify(db, &notice, &[user_id]).await.map(Some)
}
