//! The sandbox-agent-token refusal tables of the staff console: which writes take
//! [`RefuseSandboxAgent`](oxy_app_core::agent_refusal::RefuseSandboxAgent) and which
//! are decided by the environment they name.
//!
//! The extractor itself is in `oxy-app-core` (`agent_refusal`), because an
//! `oxy-app` handler (`admin::app_publish_tokens`) takes it too; the tables are the
//! console's own, so they live with the console.

/// Every route whose handler takes [`RefuseSandboxAgent`], as `(method, path
/// under /api)`: the staff console's writes to state a sandbox is not.
///
/// Two tests read this table. The route-catalog walk
/// (`route-catalog/src/tests/sandbox_agent_refusal.rs`) fails when the
/// console gains a write that is neither here nor in [`ENVIRONMENT_DECIDED`],
/// and `tests/custom_apps/sandbox_agent_token/second_refusal_writes.rs` sends
/// the token to every row with no allow-list in front.
pub const REFUSED_WRITES: &[(&str, &str)] = &[
    ("POST", "/customer-apps"),
    ("PATCH", "/customer-apps/{id}"),
    ("DELETE", "/customer-apps/{id}"),
    ("POST", "/customer-apps/{id}/publish"),
    ("DELETE", "/customer-apps/{id}/publish"),
    ("POST", "/customer-apps/{id}/rollback"),
    ("POST", "/customer-apps/{id}/environments/{name}/promote"),
    ("POST", "/customer-apps/{id}/environments/{name}/rollback"),
    ("POST", "/customer-apps/batch/publish"),
    ("POST", "/customer-apps/batch/promote-latest"),
    ("POST", "/customer-apps/batch/unpublish"),
    ("POST", "/customer-apps/batch/delete"),
    ("POST", "/customer-apps/storage/sweep"),
    ("POST", "/customer-apps/{id}/storage/delete"),
    ("POST", "/customer-apps/{id}/publishers"),
    ("DELETE", "/customer-apps/{id}/publishers/{publisher_id}"),
    // An environment's schedules switch — a sandbox's included. Turning
    // schedules on costs warehouse reads and third-party calls on a timer;
    // staff flip it, never a token (environments design §4.5, ruling 1).
    ("PUT", "/customer-apps/{id}/environments/{name}/schedules"),
    ("PUT", "/customer-apps/{id}/access"),
    // Sends as the app to the caller. A sandbox agent is not the person a
    // console test is for, and sends nothing as an app outside its sandbox.
    ("POST", "/customer-apps/{id}/notification-test"),
    // Which customer-owned hostnames serve the app's production, and the DNS
    // proof behind one: an org's configuration, not the sandbox loop's.
    ("POST", "/customer-apps/{id}/domains"),
    ("DELETE", "/customer-apps/{id}/domains/{domain_id}"),
    ("POST", "/customer-apps/{id}/domains/{domain_id}/verify"),
    ("POST", "/admin/app-publish-tokens"),
    ("POST", "/admin/app-publish-tokens/{id}/revoke"),
];

/// The console's other writes: the sandbox loop's own. Each handler decides
/// by the environment the request names, through `oxy-authz`
/// (`may_open_environment`) and its own ownership check, and refuses the
/// token anywhere but a sandbox it created — production and staging included
/// — so it takes no [`RefuseSandboxAgent`]. `(method, path, what decides)`.
///
/// A token **granted an app's staging** is admitted to that staging by two of
/// them and no more: P1 for a draft (`agent_draft::admit_agent_draft`, never a
/// promote) and C2 for a check. The other seven stay a sandbox's alone, each
/// by its own check and not by the model: E2 and E3 never create or delete
/// staging (`handlers::opens`, `ops::require_sandbox`), E4 and E5 take the
/// delete door, which refuses a token production and staging by name, and S1
/// and S2 touch a `dev-*` environment's secrets only
/// (`secrets::agent::authorize`).
pub const ENVIRONMENT_DECIDED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/customer-apps/{id}/environments",
        "E2: sandboxes::handlers admit",
    ),
    (
        "DELETE",
        "/customer-apps/{id}/environments/{name}",
        "E3: sandboxes::handlers admit, then the row lock",
    ),
    (
        "PUT",
        "/customer-apps/{id}/environments/{name}/planes",
        "E4: sandboxes::handlers admit (the delete door), then the row lock",
    ),
    (
        "POST",
        "/customer-apps/{id}/environments/{name}/reset",
        "E5: sandboxes::handlers admit (the delete door)",
    ),
    (
        "POST",
        "/customer-apps/publish",
        "P1: publish_to refuses a channel",
    ),
    (
        "POST",
        "/customer-apps/{id}/functions/{name}/runs",
        "C2: run_function_job refuses production",
    ),
    (
        "POST",
        "/customer-apps/{id}/secrets",
        "S1: secrets::agent authorize",
    ),
    (
        "DELETE",
        "/customer-apps/{id}/secrets/{key}",
        "S2: secrets::agent authorize",
    ),
];
