//! Routes that do not require authentication: health probes, auth endpoints,
//! current-user lookup, and provider-originated webhooks/callbacks (Slack,
//! Toast, GitHub, and per-app function webhooks).
//!
//! "No auth gate" here means no *user session*, not "no caller check". The
//! webhook receivers authenticate the SENDER instead, by HMAC over the raw
//! body — see `webhooks::app_function`, which fails closed twice (undeclared
//! webhook → 404, unresolvable secret → 401) precisely because it is anonymous.
//!
//! Every route here is `route_fleet`, and that is a declaration of what was
//! already true rather than a change: this file used to mount 51 routes with a
//! bare `.route(`, so every one of them took `classify`'s FleetOk default. The
//! difference is that the default is silent and a declaration is not — and
//! `route_fleet` takes a `MethodRouter<FleetState>`, so a handler added here
//! that asks for `WorkspaceManagerWorkingCopy` now fails to compile instead of
//! shipping as a route a stateless replica cannot serve.
//!
//! None of the current handlers asks for one (checked), which is why the
//! conversion is behaviour-preserving. What it does NOT fix is a handler that
//! reaches for disk without an extractor — `POST /projects/{id}/query` answers
//! `cannot resolve path '.db/'` on a replica while returning rows on the ide,
//! and stays FleetOk deliberately: `role_manifest_tests`'
//! `the_customer_app_data_plane_is_fleet_ok` pins that, and the DuckDB-`local`
//! limitation is recorded in `customer-apps-oxy-starter-fleet.flow.test.yml`.

use axum::routing::{get, post};

use crate::api::{auth, billing, healthcheck, user, webhooks};
use crate::server::api::projects;

use super::AppState;
use super::mobile::build_public_mobile_routes;
use super::role_router::RoleRouter;

pub(super) fn build_public_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet("/health", get(healthcheck::health_check))
        .route_fleet("/ready", get(healthcheck::readiness_check))
        .route_fleet("/live", get(healthcheck::liveness_check))
        .route_fleet("/version", get(healthcheck::version_info))
        // The mobile app's sign-in redeem and refresh. The personal-token
        // handshakes beside them (`oxyc login`'s exchange, the device flow, the
        // ticket redeem, the OIDC exchange, leak reports) are
        // `oxy-api-identity`'s, through `SurfaceSeams::public`.
        .merge(build_public_mobile_routes(app_state))
        .route_fleet("/auth/config", get(auth::get_config))
        .route_fleet("/auth/session", get(auth::get_session))
        .route_fleet("/auth/oauth/state", post(auth::issue_oauth_state))
        .route_fleet("/auth/google", post(auth::google_auth))
        .route_fleet("/auth/github", post(auth::github_auth))
        .route_fleet("/auth/okta", post(auth::okta_auth))
        .route_fleet("/auth/magic-link/request", post(auth::request_magic_link))
        // Frontline sign-in and the kiosk binding (`/frontline/{roster,login,
        // device,devices/bind}`) moved to the `oxy-api-frontline` sibling crate.
        // `oxy-server` merges them beside this tree through the public seam
        // (`SurfaceSeams::public`), outside the auth stack exactly as they were
        // here, and declares them FleetOk in `oxy_api_frontline::public_route_roles`.
        .route_fleet("/auth/magic-link/verify", post(auth::verify_magic_link))
        .route_fleet("/auth/return-to/validate", get(auth::validate_return_to))
        // Dev-only sign-in bypass. Public by necessity (it IS the login), but
        // 404s unless an allow-list resolves — `OXY_DEV_LOGIN_EMAILS` in any
        // build, or the staff roster on a debug build for a loopback caller —
        // and only ever issues a session for an address on it. The three rungs
        // and why they differ: `api::auth::dev_login`.
        .route_fleet(
            "/auth/dev-login",
            get(auth::dev_login_get).post(auth::dev_login),
        )
        .route_fleet("/user", get(user::get_current_user_public))
        .route_fleet("/webhooks/stripe", post(billing::webhook::stripe_webhook))
        // The GitHub App's webhook: a push to a default branch queues a commit
        // compile. Signature-verified; FleetOk because it only reads Postgres
        // and queues a task.
        .route_fleet("/webhooks/github", post(webhooks::github::github_webhook))
        .route_fleet(
            "/webhooks/toast/orders",
            post(webhooks::toast::toast_order_webhook),
        )
        // Slack-originated traffic (the webhooks, the OAuth callback, the
        // account-link landing page) is `oxy-api-slack`'s, merged beside this
        // tree through `SurfaceSeams::public`.
        // Public: Intuit redirects the bare browser here after consent. The
        // state nonce (single-use, 15-min TTL) is the CSRF/replay guard.
        .route_fleet(
            "/quickbooks/oauth/callback",
            get(crate::integrations::quickbooks::oauth::callback::callback),
        )
        // Uniform callback for every other provider. Public for the same reason:
        // the vendor redirects a bare browser here with no Oxy session, and the
        // single-use nonce — now also matched on provider — is the guard.
        // QuickBooks keeps its own path above because that URL is registered in
        // every customer's Intuit app and can never be renamed.
        .route_fleet(
            "/oauth/{provider}/callback",
            get(crate::integrations::quickbooks::oauth::callback::callback_by_slug),
        )
        // Project-scoped routes for custom-app bundles. Auth is performed
        // inline (session cookie or API key) at the customer-app gate, so these
        // sit in the public router rather than under workspace middleware. The
        // query routes (`/query`, `/semantic-query`) are `oxy-api-custom-apps`'
        // (`data_plane`), merged beside this tree through `SurfaceSeams::public`.
        // Metric-tree analysis ops for bundles — drivers / what-if / RCA /
        // opportunity sizing. Same airlayer analyses as the IDE's workspace
        // `/semantic/metric-tree*` routes, but gated for customer apps and
        // loading the layer from the compile boundary (stateless-fleet safe).
        // SDK exposes via `useMetricTree` / `useSensitivity` / `usePredict` /
        // `useExplain` / `useOpportunity` / `useDistribution` /
        // `useTimeDimensions` / `useBaseline` / `useProjection`.
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree",
            get(projects::metric_tree::get_metric_tree),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/{measure_id}/sensitivity",
            get(projects::metric_tree::get_sensitivity),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/predict",
            post(projects::metric_tree::post_predict),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/explain",
            post(projects::metric_tree::post_explain),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/opportunity",
            post(projects::metric_tree::post_opportunity),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/time-dimensions",
            get(projects::metric_tree::get_time_dimensions),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/baseline",
            post(projects::metric_tree::post_baseline),
        )
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/distribution",
            post(projects::metric_tree::post_distribution),
        )
        // Scenario forecasting's time axis: bucketed history + forward curve
        // for the levers and everything downstream. SDK exposes via
        // `useProjection` / `client.metricTree.getProjection`.
        .route_fleet(
            "/projects/{project_id}/semantic/metric-tree/projection",
            post(projects::metric_tree_projection::post_projection),
        )
        // The World Model graph, its instances and measure breakdown, and peer
        // cohorts (`/projects/{project_id}/semantic/{world-model*,cohort}`) are
        // `oxy-api-world-model`'s, beside their IDE twins, merged beside this
        // tree through `SurfaceSeams::public` with the same customer-app gate.
        // The one-shot ask's start and cancel
        // (`/projects/{project_id}/agents/{agent_id}/asks`,
        // `/projects/{project_id}/agents/asks/{run_id}/cancel`) are
        // `oxy-api-custom-apps`'s (`agent_ask`), merged beside this tree
        // through `SurfaceSeams::public`; its queued executor arrives through
        // `SurfaceSeams::tasks`. So does the agent-run SSE stream
        // (`/projects/{project_id}/agents/runs/{run_id}/events`,
        // `agent_run_stream`).
        // Phase 3 — long-running automations. POST starts, GET polls,
        // POST .../cancel stops in-flight. Persistent state in
        // `customer_app_procedure_runs` survives server restarts.
        .route_fleet(
            "/projects/{project_id}/procedures/{procedure_id}/runs",
            post(projects::automation_run::start_automation_run),
        )
        .route_fleet(
            "/projects/{project_id}/procedures/runs/{run_id}",
            get(projects::automation_run::poll_automation_run),
        )
        .route_fleet(
            "/projects/{project_id}/procedures/runs/{run_id}/cancel",
            post(projects::automation_run::cancel_automation_run),
        )
}
