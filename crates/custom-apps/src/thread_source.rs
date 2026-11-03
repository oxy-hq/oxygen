//! Which `threads.source` a custom-app ask is filed under, and so which chat
//! history a request reads: production's, or one app's staging.
//!
//! Here, below `oxy-app`, because the data plane's agent ask files its thread
//! under it and `oxy-api-custom-apps` lists the history by it.

use uuid::Uuid;

/// The `threads.source` of a production custom-app ask.
pub const PRODUCTION_THREAD_SOURCE: &str = "custom-app";

/// The `threads.source` of an ask on `app_id`'s staging host. Scoped to the
/// app, so one app's staging never reads another's. An unknown app (`None`)
/// still yields a staging source, which no production read matches.
pub fn staging_thread_source(app_id: Option<Uuid>) -> String {
    format!(
        "custom-app-staging:{}",
        app_id.map(|a| a.to_string()).unwrap_or_default()
    )
}

/// The `threads.source` of an ask on sandbox `environment` of `app_id`: the
/// sandbox's own history, never staging's or production's. Scoped to the app
/// and to the sandbox, so one developer's sandbox never reads another's.
pub fn sandbox_thread_source(app_id: Option<Uuid>, environment: &str) -> String {
    format!(
        "custom-app-sandbox:{}:{environment}",
        app_id.map(|a| a.to_string()).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_environment_files_under_its_own_source() {
        let app = Uuid::from_u128(7);
        let sources = [
            PRODUCTION_THREAD_SOURCE.to_string(),
            staging_thread_source(Some(app)),
            sandbox_thread_source(Some(app), "dev-a1"),
            sandbox_thread_source(Some(app), "dev-b2"),
            sandbox_thread_source(Some(Uuid::from_u128(8)), "dev-a1"),
        ];
        for (i, a) in sources.iter().enumerate() {
            for b in &sources[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
