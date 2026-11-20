//! Routes that do not require authentication: the OAuth callbacks. The Stripe
//! webhook (`POST /webhooks/stripe`) is `oxy-api-billing`'s since the admin
//! split's PR 9, merged beside this tree through `SurfaceSeams::public`,
//! declared `*` FleetOk there as `route_fleet` declared it here. The health probes (`/health`, `/ready`, `/live`,
//! `/version`) are `oxy-server`'s (`core_routes`, 2026-11-16), merged beside
//! this tree through `SurfaceSeams::public`. The other provider-originated receivers —
//! Slack, Toast (`oxy-api-world-model`), GitHub (`oxy-api-github`) and the
//! per-app function webhooks (`oxy-api-custom-apps`) — are merged beside this
//! tree through `SurfaceSeams::public`.
//!
//! "No auth gate" here means no *user session*, not "no caller check". The
//! webhook receivers authenticate the SENDER instead, by HMAC over the raw
//! body — see `oxy_api_custom_apps::webhooks::app_function`, which fails closed
//! twice (undeclared webhook → 404, unresolvable secret → 401) precisely
//! because it is anonymous.
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

use axum::routing::get;

use super::AppState;
use super::role_router::RoleRouter;

pub(super) fn build_public_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        // The health probes (`/health`, `/ready`, `/live`, `/version`) are
        // `oxy-server`'s (`core_routes`, 2026-11-16), merged beside this tree
        // through `SurfaceSeams::public`, declared `*` FleetOk there.
        // Signing in (`/auth/{config,session,oauth/state,google,github,okta,
        // magic-link/*,return-to/validate,dev-login}`), the mobile app's redeem
        // and refresh (`/auth/mobile/{exchange,refresh}`) and `GET /user` are
        // `oxy-api-identity`'s (`sign_in`, `user`), merged beside this tree
        // through `SurfaceSeams::public` (2026-11-15), as are the personal-token
        // handshakes (`oxyc login`'s exchange, the device flow, the ticket
        // redeem, the OIDC exchange, leak reports). The frontline PIN sign-in
        // and kiosk binding are `oxy-api-frontline`'s, through the same seam.
        // Stripe's webhook (`POST /webhooks/stripe`) is `oxy-api-billing`'s
        // (admin split, PR 9), merged beside this tree through
        // `SurfaceSeams::public`, declared `*` FleetOk there.
        // The GitHub App's webhook (`POST /webhooks/github`: a push to a
        // default branch queues a commit compile) is `oxy-api-github`'s
        // (`webhook`, 2026-11-15), merged beside this tree through
        // `SurfaceSeams::public`, declared `*` FleetOk there.
        // Toast's order webhook (`POST /webhooks/toast/orders`) is
        // `oxy-api-world-model`'s (`toast_webhook`, 2026-11-15), beside the live
        // feed it publishes to, merged beside this tree through
        // `SurfaceSeams::public`, declared `*` FleetOk there.
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
    // query routes (`/query`, `/semantic-query`) and the metric tree's
    // analysis ops (`/semantic/metric-tree*`, left 2026-11-14) are
    // `oxy-api-custom-apps`' (`data_plane`), merged beside this tree through
    // `SurfaceSeams::public`.
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
    // `agent_run_stream`), and the automation run's start, poll and cancel
    // (`/projects/{project_id}/procedures/…`, `automation_run`), whose
    // executor and TTL sweep arrive through `SurfaceSeams::{tasks,
    // background}`.
}
