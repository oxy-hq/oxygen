//! Subpath bundle serving for custom apps.
//!
//! Serves custom-app frontends at `GET /customer-apps/{uuid}` and
//! `GET /customer-apps/{uuid}/{*rest}`, gating each request with cookie or
//! bearer auth and an org-membership check.
//!
//! Every app is served from the build store (`oxy_custom_apps::source::AppSource`
//! has one variant): the channel the request resolves to names a build, and
//! `sources::serve_from_s3_build` reads that build's objects through the
//! in-memory bundle cache. `oxyc publish` is the only way bytes get there.
//!
//! Authentication failures redirect to `{base}/login?return_to=<url>` rather
//! than 401 so unauthenticated visitors land in the magic-link flow
//! automatically.
//!
//! ## Authentication ordering
//!
//! Auth is checked **before** any DB lookup of the requested app. This
//! prevents an unauthenticated probe from learning whether a UUID is a real
//! registered app (real → 302 redirect; fake → 404 leaks the existence
//! signal). Order is: authenticate → load user → load app → check membership.
//!
//! ## Per-request DB load
//!
//! A Next.js bundle triggers 30–100 asset requests per page load, each
//! routed through this handler, so anything uncached here is multiplied by
//! ~100 on every click. **The warm steady state is now zero DB round-trips
//! per asset.** Each step of the resolution chain is cached in process with
//! a 60-second TTL:
//!
//! | Step | Cache |
//! | ---- | ----- |
//! | user, keyed by `oxy_custom_apps::auth::user_cache_key` (id; address only for a provider identity) | `oxy_custom_apps::cache::cached_user` |
//! | access check `(user_id, app_id)` | `custom_apps_auth`, dropped by `invalidate_access_cache` |
//! | platform standing | `oxy_server_authz::globals` |
//! | org-by-slug + app-by-`(org_id, slug)` | `oxy_custom_apps::cache::cached_app_resolution` |
//! | `app_builds` row (S3 source) | `oxy_custom_apps::cache::cached_build`, see `sources.rs` |
//!
//! Asset *bytes* come from the LRU in `custom_apps_bundle_cache`, which also
//! remembers absences, so neither the SPA fallback nor the pre-compressed
//! `.br` probe re-hits the store.
//!
//! **Before adding a step to this chain, cache it.** Uncached, it costs ~100
//! queries per page load on its own — that is exactly how the previous three
//! lookups came to cost 300.

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};
use sea_orm::DatabaseConnection;
use tracing::Instrument;
use uuid::Uuid;

use oxy_app_core::custom_app_env_request::request_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::custom_domain_host::CustomDomainCtx;

use oxy_app_core::function_seam::{FunctionQueryExecutor, FunctionRequest};
use oxy_custom_apps::env_resolve::{EnvironmentBuilds, resolve_environment};

mod headers;
mod live_build;
use oxy_custom_apps::resolve;
pub(crate) mod rewrite;
mod sources;

use headers::*;
use resolve::{AppRequest, Refusal, resolve_app_request};
use sources::*;

pub(crate) use sources::serve_from_s3_build;

/// 32 MiB ceiling on an Oxy Function invocation's request body.
///
/// Function POSTs are normally small JSON. Anything larger is almost certainly
/// user-uploaded media, which belongs in `ctx.storage` via a presigned URL rather
/// than buffered through this process — one serve instance hosts many apps.
/// (Moved here from the removed v0 reverse proxy, where it also bounded proxied
/// bodies; functions are now its only use.)
pub(crate) const FUNCTION_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// One route, any pod. `serve_dispatch` answers everything under
/// `/customer-apps/{*path}`: bundle bytes from S3, and `POST .../fn/<name>`,
/// which EXECUTES an Oxy Function. Both are `FleetOk`, so a custom app — its
/// pages and its backend — does not depend on the Factory.
///
/// The function entry is stated on its own although the wildcard already
/// covers it. It was `IdeOnly` while a function read `config.yml` and the
/// semantic model off the working copy; both now come from the promoted
/// revision (`custom_apps_gates::build_project_context_with_role`,
/// `host/semantic_op.rs`). A literal entry outranks any broader pattern, so a
/// future `IdeOnly` wildcard cannot silently put every app's backend back
/// behind one pod. The invocations a replica cannot run — a workspace with
/// nothing compiled, or one whose database is a file in the checkout — are
/// replayed to the Factory by the handler itself
/// (`server::invocation_placement`).
///
/// Declared here rather than at a mount because `/customer-apps/{*path}` sits
/// on the outer router in `serve.rs`, outside `RoleRouter` — the same shape
/// `agentic_http::router_roles()` uses.
pub fn serve_dispatch_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "*",
            path: "/customer-apps/{org}/{app}/fn/{name}",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "*",
            path: "/customer-apps/{*path}",
            role: RouteRole::FleetOk,
        },
    ]
}

/// Single entry point for `GET /customer-apps/{*path}`. Decides between
/// the legacy uuid form (redirects to the canonical pretty URL) and the
/// new pretty form (`<org_slug>/<app_slug>/<rest>`).
///
/// Auth lives inside [`serve_pretty`] so the legacy-uuid redirect path
/// can still bounce anonymous visitors through `/login?return_to=...`
/// before they ever learn whether a given uuid is a real app.
pub async fn serve_dispatch(Path(path): Path<String>, request: axum::extract::Request) -> Response {
    let (parts, body) = request.into_parts();
    // Set by the host dispatcher when the request arrived on a verified
    // customer-owned domain (`oxy_app_core::custom_domain_host`).
    let custom_domain = parts.extensions.get::<CustomDomainCtx>().cloned();
    let headers = parts.headers;
    let uri = parts.uri;
    let method = parts.method;

    // A sandbox agent token reaches one shape on this tree, `/fn` of a `dev-*`
    // sandbox on the product host. Asked first, before any authentication, so
    // nothing below has to remember the kind exists. It is refused only for
    // presenting the token, so the refusal is in the token's one body: the
    // `404` that `/fn` answers for an app or a sandbox that is not its own.
    if oxy_app::surface::serve_tree_refuses(&method, &headers, &path) {
        return oxy_app_core::agent_body::Refusal::not_found().into_response();
    }

    // Strip the leading slash that axum hands us for a `{*path}` capture
    // when the URL is `/customer-apps/`. Split lazily into the first two
    // segments + the remainder; an empty path means /customer-apps/ which
    // has nothing to dispatch on.
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let mut path_parts = trimmed.splitn(2, '/');
    let first = path_parts.next().unwrap_or("");
    let rest_after_first = path_parts.next().unwrap_or("");

    // Legacy uuid form: `/customer-apps/<uuid>/<rest>` → 301 to canonical.
    if let Ok(uuid) = first.parse::<Uuid>() {
        return redirect_legacy_uuid(uuid, rest_after_first, &headers, &uri).await;
    }

    // Pretty form: `/customer-apps/<org_slug>/<app_slug>/<rest>`. Need to
    // split `rest_after_first` once more to get the app slug.
    let mut rest_parts = rest_after_first.splitn(2, '/');
    let app_slug = match rest_parts.next() {
        Some(s) if !s.is_empty() => s,
        _ => {
            // We have an org but no app component — 404.
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let rest = rest_parts.next().unwrap_or("").to_string();

    // A custom domain serves its own app and nothing else. The dispatcher only
    // ever rewrites to that app, so this is the guard that keeps it so.
    if let Some(domain) = &custom_domain
        && (domain.org_slug != first || domain.app_slug != app_slug)
    {
        return no_store_404();
    }

    // Oxy Functions route invocation: `POST .../fn/<name>`. Dispatched
    // before `serve_pretty`'s static-bundle logic — see
    // internal-docs/customer-apps-functions.md §11.10.
    if let Some(function_name) = rest.strip_prefix("fn/") {
        if function_name.is_empty() || function_name.contains('/') {
            return StatusCode::NOT_FOUND.into_response();
        }
        let body_bytes = match axum::body::to_bytes(body, FUNCTION_BODY_LIMIT).await {
            Ok(b) => b,
            Err(_) => {
                // The bare status, but for a sandbox agent token (its one body).
                let refused = StatusCode::BAD_REQUEST;
                return oxy_app_core::agent_body::status_response(&headers, refused);
            }
        };
        // The function query executor is injected at the serve router (an
        // Extension layer) so this runtime never imports `projects::query`.
        // Absent only if the router wiring regressed — fail closed.
        let query_exec = match parts
            .extensions
            .get::<std::sync::Arc<dyn FunctionQueryExecutor>>()
            .cloned()
        {
            Some(exec) => exec,
            None => {
                tracing::error!(
                    "custom-app function invoked but no FunctionQueryExecutor is wired into the serve router"
                );
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        // Layer-1 preagg cache, injected alongside the query executor at the
        // serve router. Absent (default = no cache) in any composition that
        // does not run a rebuild worker, in which case `ctx.semantic` compiles
        // straight to warehouse SQL — the same posture the CLI takes.
        let preagg = parts
            .extensions
            .get::<oxy_app_core::preagg_cache_ctx::PreaggCacheCtx>()
            .cloned()
            .unwrap_or_default();
        return crate::custom_apps_functions::handle_function_request(
            first,
            app_slug,
            function_name,
            method,
            uri,
            headers,
            body_bytes,
            query_exec,
            preagg,
        )
        .await;
    }

    let request = ServeRequest {
        method,
        headers,
        uri,
        custom_domain,
    };
    serve_pretty(first, app_slug, rest, request, body).await
}

/// The request as the bundle path reads it.
pub(crate) struct ServeRequest {
    pub(crate) method: axum::http::Method,
    pub(crate) headers: HeaderMap,
    pub(crate) uri: Uri,
    /// `Some` when the request arrived on a verified custom domain.
    pub(crate) custom_domain: Option<CustomDomainCtx>,
}

/// The custom-app serve path, phase by phase.
///
/// The HTTP layer already produces one `GET /customer-apps/{*path}` span, and
/// in prod it reports a p95 of 262ms
/// (`internal-docs/2026-09-09-hyperdx-observability-findings.md` §8) with
/// **nothing underneath it** — so the one question that span raises is the one
/// it cannot answer. The phases below are each a child span because each is a
/// different failure and a different fix: an auth round trip, a user lookup, a
/// two-query app resolution, an access check, and the byte fetch itself. The
/// user lookup and the app resolution only produce a span on a cache miss, so
/// their presence in a trace is itself the diagnosis. The status code is not
/// repeated here: the parent HTTP span carries it on every path, early exits
/// included, which a field recorded at the bottom of this function would not.
///
/// The stages, each over one [`ServeContext`]: environment → principal, app
/// and access (the shared resolver) → reserved `__oxy/` names → build → bytes
/// → record. The order is the contract: authentication before any lookup,
/// the reserved namespace after the gate and before the bundle, recording
/// last so every answer is counted.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    target = "custom_apps_serve",
    name = "custom_app_serve",
    skip_all,
    fields(
        org_slug = %org_slug,
        app_slug = %app_slug,
        route = %rest,
        app_id = tracing::field::Empty,
        source = tracing::field::Empty,
    )
)]
pub(crate) async fn serve_pretty(
    org_slug: &str,
    app_slug: &str,
    rest: String,
    request: ServeRequest,
    body: axum::body::Body,
) -> Response {
    let ctx = match ServeContext::prepare(org_slug, app_slug, rest, request).await {
        Ok(ctx) => ctx,
        Err(answered) => return answered,
    };
    if let Some(answered) = ctx.answer_reserved(body).await {
        return answered;
    }
    let response = ctx.dispatch().await;
    ctx.record(response)
}

/// What every stage after the prelude reads: the request as it arrived, the
/// resolved principal and app, and the runtime config the bundle is served
/// with.
struct ServeContext {
    /// Wall clock for the wide event emitted at the bottom. Started before the
    /// auth round-trip on purpose: a login check that has gone slow IS the app
    /// being slow from the viewer's seat, and a timer that excluded it would
    /// report health the viewer does not have.
    started: std::time::Instant,
    environment: AppEnvironment,
    request: AppRequest,
    rest: String,
    method: axum::http::Method,
    headers: HeaderMap,
    uri: Uri,
    runtime: AppRuntimeConfig,
    /// The app's own origin (a platform subdomain or a custom domain), where
    /// the app owns `/`, as opposed to the product host's subpath.
    on_app_host: bool,
}

impl ServeContext {
    /// Stages 0-3: environment, principal, app, access, source. Every refusal
    /// is answered here in the bundle path's own voice ([`refuse`]), and a
    /// platform 500 against a known app is recorded for the SLI.
    async fn prepare(
        org_slug: &str,
        app_slug: &str,
        rest: String,
        serve_request: ServeRequest,
    ) -> Result<Self, Response> {
        let ServeRequest {
            method,
            headers,
            uri,
            custom_domain,
        } = serve_request;
        let started = std::time::Instant::now();
        // 0. Which environment of the app this request addresses: the host
        //    label, or `X-Oxy-App-Env` on a bearer request. A header that
        //    cannot be honoured is a 400 — never a silent production serve.
        //    Pure header work, so it reveals nothing about which apps exist.
        let environment = request_environment(&headers).map_err(|e| e.into_response())?;
        // 1-2. Authenticate FIRST, then user → rows → access, every step from
        //    the 60 s caches (`resolve`). No DB work happens before we know
        //    the caller has a valid session, so an unauthenticated probe can't
        //    distinguish a real registered (org, app) pair (302 redirect) from
        //    a fake one (404).
        let request = resolve_app_request(
            &headers,
            org_slug,
            app_slug,
            oxy_auth::token::SandboxAgent::Refuse,
            &environment,
        )
        .await
        .map_err(|refusal| {
            let at = RefusedAt {
                org_slug,
                app_slug,
                rest: &rest,
                headers: &headers,
                uri: &uri,
                custom_domain: custom_domain.as_ref(),
            };
            refuse(refusal, at, started)
        })?;
        // 3. The source facade. The per-app source is recorded at register
        //    time; a row still carrying a removed type (`v0`, `local`) fails to
        //    parse here and answers a recorded 500 — see `custom_apps_source`
        //    for why those rows are not migrated.
        if let Err(e) = oxy_custom_apps::source::AppSource::from_model(&request.app) {
            tracing::error!("Bad source config for custom app {}: {e}", request.app.id);
            EarlyExit::new(&request.app, request.user.id, &headers, &rest, started)
                .record(StatusCode::INTERNAL_SERVER_ERROR);
            return Err(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        let runtime = AppRuntimeConfig::from_app(
            &request.app,
            request.environments.production,
            &request.org.slug,
            &environment,
        );
        let on_app_host = custom_domain.is_some() || on_custom_app_subdomain(&headers);
        Ok(Self {
            started,
            environment,
            request,
            rest,
            method,
            headers,
            uri,
            runtime,
            on_app_host,
        })
    }

    /// Stage 3a: the platform-reserved `__oxy/` namespace, answered by us
    /// rather than by the bundle. Deliberately HERE — after authentication and
    /// the access check, before the source dispatch — so it inherits the app's
    /// exact gate: the same viewers who can load the bundle can register its
    /// worker and post its telemetry, and nobody else can. It also means these
    /// paths work identically on the subpath and subdomain surfaces with no
    /// CORS involved, because they *are* the app's origin.
    ///
    /// Not everything under the prefix is answered here — the asset manifest
    /// is a real object inside the build — but everything under it is
    /// *classified* here, so a name nobody claims 404s rather than reaching
    /// the bundle. That is what makes the namespace reserved rather than
    /// merely conventional. `None` means "not reserved, serve the bundle".
    async fn answer_reserved(&self, body: axum::body::Body) -> Option<Response> {
        let name = reserved_platform_path(&self.rest)?;
        let app = &self.request.app;
        let user = &self.request.user;
        Some(match classify_reserved(name, &self.method) {
            Reserved::ServiceWorker => service_worker_for(&self.runtime, self.on_app_host),
            Reserved::WebManifest => {
                web_manifest_for(&self.request.db, &self.runtime, app, self.on_app_host).await
            }
            Reserved::MonogramIcon => monogram_icon_for(app),
            // Answered before view and SLI recording, like the beacon: a tab
            // polling for a publish is not a page view and not traffic.
            Reserved::LiveBuild => {
                live_build::live_build_for(
                    &self.request.db,
                    app,
                    &self.environment,
                    &self.request.environments,
                )
                .await
            }
            Reserved::Beacon => {
                ingest_beacon(
                    body,
                    &self.headers,
                    app.id,
                    app.org_id,
                    user.id,
                    user.email.as_deref().unwrap_or(""),
                    &self.environment,
                    // A page shown from a prefetch reports its own open; it is
                    // written exactly as a served page's would be.
                    |referrer| Some(self.record_view(referrer)),
                )
                .await
            }
            // A real object inside the build, written at publish — fall
            // through to the ordinary dispatch so it rides the same LRU,
            // pre-compression, and absence caching as every other asset.
            Reserved::BuildObject => return None,
            Reserved::Unknown => crate::client::reserved_not_found(),
        })
    }

    /// Stages 4-5: the build this request serves, then its bytes. The app id
    /// and the backend that answered go on the parent span: "which app, served
    /// from where" is how an operator slices a latency or error question before
    /// looking at any single request.
    async fn dispatch(&self) -> Response {
        let app = &self.request.app;
        let span = tracing::Span::current();
        span.record("app_id", tracing::field::display(app.id));
        span.record("source", "s3");
        let dispatch_span = tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_dispatch"
        );
        async {
            let (build_pk, label) = build_to_serve(
                &self.request.db,
                app,
                &self.environment,
                &self.request.environments,
            )
            .await;
            match build_pk {
                Some(build_pk) => self.serve_build(build_pk).await,
                None => {
                    // Post-retirement: the legacy state-dir serve is gone. An
                    // s3-source app with no build pointer hasn't been published
                    // through the new pipeline yet (`oxyc publish`).
                    tracing::warn!(
                        "app {}: no build for {label} — not yet published via `oxyc publish`",
                        app.id
                    );
                    no_store_404()
                }
            }
        }
        .instrument(dispatch_span)
        .await
    }

    /// Stage 5: the bytes of one build.
    async fn serve_build(&self, build_pk: Uuid) -> Response {
        // The runtime config was built in `prepare`, before the channel was
        // known, so its `buildId` defaulted to the PUBLISHED build. A staff
        // draft preview of a published app would then serve draft bytes and
        // inject the published id: an error thrown by draft code gets
        // attributed to the wrong build, `resolve_stack` fetches that build's
        // maps, and the lookups return plausible-but-wrong files and lines —
        // reported as `stack_resolved: true`, which is the one case that flag
        // exists to prevent. Draft preview is exactly when someone is reading
        // a stack.
        let mut runtime = self.runtime.clone();
        runtime.build_id = build_pk.to_string();
        // `?v=` enters the cache decision here. The query never reaches
        // `rest` (a path capture), so it is read off the URI and compared
        // downstream against the build row actually loaded for this request.
        serve_from_s3_build(
            &self.request.db,
            self.request.app.id,
            build_pk,
            &self.rest,
            requested_build_version(self.uri.query()),
            &runtime,
            &self.headers,
        )
        .await
    }

    /// Stage 6: one wide event per served request, and for a rendered
    /// browser navigation the view event and its session cookie.
    ///
    /// The invariant: a non-2xx is not a view, whoever produced it. `304` is
    /// the one exception — the browser re-rendered the page from its cache,
    /// which is a view (`html_response`'s `if_none_match` produces one). What
    /// that buys: an app that is registered but has nothing to serve — no
    /// `oxyc publish` yet — stops recording a view for every hit on its 404,
    /// a state customers sit in. This gates the `Set-Cookie` too, so a visitor
    /// whose first hit is a non-2xx starts a fresh session on their next
    /// navigation — harmless, the tracking cookie is analytics-only and
    /// separate from `oxy_session`.
    ///
    /// Speculative prefetches are excluded from both: the HQ launcher warms an
    /// app on card hover (`prefetchApp`), and counting those would make hover
    /// traffic indistinguishable from real load — and mint a session id at
    /// hover time that the real navigation would then inherit.
    fn record(self, mut response: Response) -> Response {
        if is_speculative_request(&self.headers) {
            return response;
        }
        let status = response.status();
        let is_html_request = is_html_navigation(&self.rest);
        self.record_serve_event(status, is_html_request);
        let rendered = status.is_success() || status == StatusCode::NOT_MODIFIED;
        if is_html_request && rendered {
            let referrer = self
                .headers
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok());
            let set_cookie = self.record_view(referrer);
            if let Ok(hv) = axum::http::HeaderValue::from_str(&set_cookie) {
                response.headers_mut().append(header::SET_COOKIE, hv);
            }
        }
        response
    }

    /// Every request, not just the HTML ones the view recorder counts. This
    /// is the denominator the availability SLI is computed from, and an SLI
    /// that only saw successful page loads would be a tautology. Assets are
    /// recorded and then excluded from the ratio at query time (see
    /// `availability_sql`), so an asset failure is diagnosable without being
    /// able to drown out a shell failure.
    fn record_serve_event(&self, status: StatusCode, is_html: bool) {
        oxy_custom_apps::telemetry::record_serve(oxy_custom_apps::telemetry::ServeEvent {
            org_id: self.request.app.org_id,
            app_id: self.request.app.id,
            build_id: None,
            request_id: oxy_shared::utils::request_id::from_headers(&self.headers),
            session_id: None,
            user_id: self.request.user.id,
            is_html,
            route: &self.rest,
            status: status.as_u16(),
            duration_ms: self.started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            environment: &self.environment.name(),
        });
    }

    /// Queue the view-event row for a rendered navigation and return the
    /// `Set-Cookie` that names its session. The cookie is minted here,
    /// synchronously; the row goes onto the bounded usage queue (a `try_send`:
    /// no spawn, no connection, no read on this path), and a full queue drops
    /// it. Losing a row on overload or crash is the documented acceptable
    /// failure mode for tracking-grade data.
    ///
    /// `referrer` is the page's: the `Referer` of the HTML request when the
    /// server records the view, or the one the page reports when it records
    /// its own open through the beacon (whose own `Referer` is the app).
    fn record_view(&self, referrer: Option<&str>) -> String {
        let headers = &self.headers;
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(':')
            .next()
            .unwrap_or("");
        let secure = self.uri.scheme_str() == Some("https")
            || headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.eq_ignore_ascii_case("https"))
                .unwrap_or(false);
        let (session_id, set_cookie) =
            oxy_custom_apps::tracking::session_id_for_serve(headers, secure);
        let sanitized_referrer = oxy_custom_apps::tracking::sanitize_referrer(referrer, host);
        let user_agent_class = classify_user_agent(headers.get(header::USER_AGENT));
        // The recorder needs the app row to resolve the viewer's role snapshot.
        // Hand it the one already resolved (and process-cached) rather than
        // making it refetch by id — the read is free here and isn't there.
        let recorded_app = self.request.app.clone();
        let user_id = self.request.user.id;
        let user_email = self.request.user.email.clone();
        let source_label = if self.on_app_host {
            "subdomain"
        } else {
            "subpath"
        }
        .to_string();
        let view_environment = self.environment.name();
        oxy_custom_apps::tracking::record_view(
            recorded_app,
            user_id,
            user_email,
            session_id,
            sanitized_referrer,
            user_agent_class,
            source_label,
            view_environment,
        );
        set_cookie
    }
}

/// Answer a refusal from the shared resolver in the bundle path's own voice:
/// an unauthenticated browser is sent to login (never a 401 — the magic-link
/// flow picks it up), a credential naming nobody likewise, an unknown app is a
/// `no_store_404`, and a 403 or a platform 500 against a known app is recorded
/// for the SLI before it is answered.
/// Where a refusal happened: the request, as [`refuse`] needs it.
struct RefusedAt<'a> {
    org_slug: &'a str,
    app_slug: &'a str,
    rest: &'a str,
    headers: &'a HeaderMap,
    uri: &'a Uri,
    custom_domain: Option<&'a CustomDomainCtx>,
}

impl RefusedAt<'_> {
    /// An unauthenticated visitor: to the product login, or — on a custom
    /// domain, which the login's cookie never reaches — into the sign-in
    /// handoff, starting from the path the visitor asked for on the domain.
    fn to_sign_in(&self) -> Response {
        match self.custom_domain {
            Some(domain) => {
                let path = format!("/{}", self.rest);
                let rest = match self.uri.query() {
                    Some(q) => format!("{path}?{q}"),
                    None => path,
                };
                crate::custom_domains::handoff::start(self.headers, domain, &rest)
            }
            None => redirect_to_login(self.headers, self.uri).into_response(),
        }
    }
}

fn refuse(refusal: Refusal, at: RefusedAt<'_>, started: std::time::Instant) -> Response {
    let RefusedAt {
        org_slug,
        app_slug,
        rest,
        headers,
        ..
    } = at;
    match refusal {
        Refusal::Unauthenticated(error) => {
            // Surface auth failures so an operator can tell apart "no cookie"
            // (browser never logged in) vs "stale JWT" vs "wrong domain
            // scope". Without this the only symptom is a silent redirect to
            // login and there's no way to know why from outside the server.
            // `info`, not `warn`: an unauthenticated hit is the client doing
            // something ordinary (a service-worker update check, a
            // `version.json` poll, an expired session), and at `warn` it was a
            // third of prod's warnings. The fields keep the diagnosis.
            let cookie_present = headers
                .get(header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.contains("oxy_session="))
                .unwrap_or(false);
            let auth_header_present = headers.get(header::AUTHORIZATION).is_some();
            tracing::info!(
                target: "custom_apps_serve",
                org_slug = %org_slug,
                app_slug = %app_slug,
                cookie_present,
                auth_header_present,
                error = %error,
                "auth failed, redirecting to login"
            );
            at.to_sign_in()
        }
        Refusal::UnknownUser => at.to_sign_in(),
        Refusal::NotFound => no_store_404(),
        Refusal::Forbidden(denied) => {
            // A 403 is not an app fault and does not dent the SLI (see
            // `is_app_fault`), but it is still traffic this app served and an
            // operator asking "who is being turned away" has nowhere else to
            // look.
            let exit = EarlyExit::new(&denied.app, denied.user_id, headers, rest, started);
            exit.record(StatusCode::FORBIDDEN);
            (
                StatusCode::FORBIDDEN,
                "You don't have access to this custom app.",
            )
                .into_response()
        }
        Refusal::Internal(failure) => {
            // A 500 the PLATFORM produced, against the app when the chain had
            // already resolved one: the SLI built to catch platform failure
            // must see the platform's own failures.
            if let (Some(app), Some(user_id)) = (failure.app.as_ref(), failure.user_id) {
                EarlyExit::new(app, user_id, headers, rest, started)
                    .record(StatusCode::INTERNAL_SERVER_ERROR);
            }
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The build this request serves, and a label for the log when there is none.
///
/// **Production** serves the published build, or the staging (draft) build for
/// an app never promoted — there is nothing else to show — unless a sandbox
/// agent token published that draft ([`unpromoted_build`]). It never serves a
/// draft on request: the draft is the staging environment's, and it is read on
/// the staging host only. (The staff-only `oxy_preview_draft` cookie that once
/// flipped production to the draft is retired; a request still carrying one is
/// served exactly what everyone else is.)
///
/// **Any other environment** serves its own build, and nothing else: a staging
/// host never falls back to production's build. A sandbox's is read from its
/// own row, uncached ([`sandbox_build`]).
async fn build_to_serve(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    environment: &AppEnvironment,
    environments: &EnvironmentBuilds,
) -> (Option<Uuid>, String) {
    if matches!(environment, AppEnvironment::Dev { .. }) {
        return (
            sandbox_build(db, app, environment).await,
            format!("the {environment} environment"),
        );
    }
    if *environment != AppEnvironment::Production {
        return (
            environments.resolve(environment).build_id,
            format!("the {environment} environment"),
        );
    }
    let channel = resolve_channel(app.published_at.is_some());
    use oxy_custom_apps::sync::Channel;
    let build = match channel {
        Channel::Draft => unpromoted_build(db, app, environments.staging).await,
        Channel::Published => environments.production,
    };
    (build, format!("the {channel:?} channel"))
}

/// What production serves for an app with no production build of its own:
/// staging's build — unless a sandbox agent token published it. Nobody
/// approved that build, so production serves nothing rather than fall back to
/// it (`custom_apps_agent_built`). A failed read serves nothing too.
async fn unpromoted_build(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    staging: Option<Uuid>,
) -> Option<Uuid> {
    match oxy_custom_apps::agent_built::production_fallback(db, staging).await {
        Ok(build) => build,
        Err(e) => {
            tracing::error!(
                "Failed to read who published the draft of custom app {}: {e}",
                app.id
            );
            None
        }
    }
}

/// The build a sandbox serves: its own row, read on every request rather than
/// from the cached [`EnvironmentBuilds`], so a publish to it or a delete of it
/// shows at once on every replica. A failed read serves nothing (the request
/// answers 404) rather than another environment's build.
async fn sandbox_build(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    environment: &AppEnvironment,
) -> Option<Uuid> {
    match resolve_environment(db, app, environment).await {
        Ok(resolved) => resolved.build_id,
        Err(e) => {
            tracing::error!(
                "Failed to read the {environment} environment of custom app {}: {e}",
                app.id
            );
            None
        }
    }
}

/// A serve event for a request that fails BEFORE reaching the normal
/// recording point at the bottom of `serve_pretty`.
///
/// The two 500s the platform itself produces — an access-check DB failure and a
/// bad source config — returned early, so the SLI that exists to catch platform
/// failure never saw the platform's own failures. One struct rather than a
/// seven-argument function so the `kind`/`outcome` classification stays in one
/// place and a call site reads as what it records.
struct EarlyExit<'a> {
    app: &'a entity::apps::Model,
    user_id: Uuid,
    headers: &'a HeaderMap,
    rest: &'a str,
    started: std::time::Instant,
}

impl<'a> EarlyExit<'a> {
    fn new(
        app: &'a entity::apps::Model,
        user_id: Uuid,
        headers: &'a HeaderMap,
        rest: &'a str,
        started: std::time::Instant,
    ) -> Self {
        Self {
            app,
            user_id,
            headers,
            rest,
            started,
        }
    }

    fn record(self, status: StatusCode) {
        // The same header read `serve_pretty` made; an unhonourable header
        // never gets this far, so the fallback is unreachable rather than a
        // guess.
        let environment = request_environment(self.headers)
            .unwrap_or(AppEnvironment::Production)
            .name();
        oxy_custom_apps::telemetry::record_serve(oxy_custom_apps::telemetry::ServeEvent {
            org_id: self.app.org_id,
            app_id: self.app.id,
            build_id: None,
            request_id: oxy_shared::utils::request_id::from_headers(self.headers),
            session_id: None,
            user_id: self.user_id,
            is_html: is_html_navigation(self.rest),
            route: self.rest,
            status: status.as_u16(),
            duration_ms: self.started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            environment: &environment,
        });
    }
}

/// The tail of a request path inside the platform-reserved namespace, or `None`
/// when the request is for the app's own files.
///
/// Case-insensitive on the prefix for the same reason `is_server_only_path` is:
/// a case-insensitive filesystem would otherwise resolve `__OXY/sw.js` to a real
/// object on one platform and not another.
fn reserved_platform_path(rest: &str) -> Option<&str> {
    let trimmed = rest.trim_start_matches('/');
    let prefix = oxy_custom_apps::asset_manifest::RESERVED_PLATFORM_PREFIX;
    if trimmed.len() < prefix.len() {
        return None;
    }
    let (head, tail) = trimmed.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

/// What a request inside the platform-reserved `__oxy/` namespace is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reserved {
    /// `GET __oxy/sw.js` — the platform service worker.
    ServiceWorker,
    /// `POST __oxy/beacon` — the client runtime's telemetry batch.
    Beacon,
    /// `GET __oxy/manifest.webmanifest` — the synthesised web app manifest.
    WebManifest,
    /// `GET __oxy/icon.svg` — the platform monogram, the icon every app has
    /// whether or not its author declared one.
    MonogramIcon,
    /// `GET __oxy/build` — the build this app serves now, polled by a
    /// long-open tab to notice a publish. See `live_build`.
    LiveBuild,
    /// A platform object that lives *inside the build* rather than being
    /// synthesised here — today only the asset manifest. Falls through to the
    /// ordinary source dispatch.
    BuildObject,
    /// Anything else. 404, never a fall-through to the bundle: reserving the
    /// prefix is what stops an app serving its own file at a platform URL, and
    /// a miss that fell through would give one back.
    Unknown,
}

/// Classify one reserved-namespace request. Pure, so the routing table is
/// testable without a database, a build store, or a request body — which is the
/// whole reason it is separated from the dispatch that acts on it.
///
/// Method is part of the decision rather than checked afterwards: `POST
/// __oxy/sw.js` is not "the worker, wrong method", it is a request for something
/// that does not exist, and answering it the same way as any other unknown
/// reserved path keeps the surface from advertising which verbs it takes.
fn classify_reserved(name: &str, method: &axum::http::Method) -> Reserved {
    let prefix = oxy_custom_apps::asset_manifest::RESERVED_PLATFORM_PREFIX;
    let sw = crate::client::SERVICE_WORKER_PATH.trim_start_matches(prefix);
    let beacon = crate::client::BEACON_PATH.trim_start_matches(prefix);
    let manifest = oxy_custom_apps::asset_manifest::ASSET_MANIFEST_PATH.trim_start_matches(prefix);
    let webmanifest = crate::client::WEB_MANIFEST_PATH.trim_start_matches(prefix);
    let monogram = crate::client::MONOGRAM_ICON_PATH.trim_start_matches(prefix);
    let live_build = crate::client::LIVE_BUILD_PATH.trim_start_matches(prefix);

    match (name, method) {
        (n, &axum::http::Method::GET) if n == sw => Reserved::ServiceWorker,
        (n, &axum::http::Method::POST) if n == beacon => Reserved::Beacon,
        (n, &axum::http::Method::GET) if n == webmanifest => Reserved::WebManifest,
        (n, &axum::http::Method::GET) if n == monogram => Reserved::MonogramIcon,
        (n, &axum::http::Method::GET) if n == live_build => Reserved::LiveBuild,
        (n, &axum::http::Method::GET) if n == manifest => Reserved::BuildObject,
        _ => Reserved::Unknown,
    }
}

/// Serve the platform worker, scoped as wide as this surface needs and no wider.
///
/// On a **custom-app subdomain** the whole origin is one app and the page sits
/// at `/`, while the bundle's assets keep the `/customer-apps/<org>/<app>/`
/// prefix (the host dispatcher passes those through rather than double-prefixing
/// them). A worker scoped to that prefix would never control the page, so the
/// origin has to permit `/`.
///
/// On the **admin host** it must not: `/` there is the Oxy console, and a custom
/// app's worker claiming it would intercept the whole SPA. `runtime.js` computes
/// the same answer client-side from the document's own path; this header is the
/// half that holds if it ever gets it wrong.
fn service_worker_for(runtime: &AppRuntimeConfig, on_app_host: bool) -> Response {
    let allowed = if on_app_host { "/" } else { &runtime.base_path };
    crate::client::service_worker_response(allowed)
}

/// The `icons[]` for one app: its own mark when it has a usable one, and the
/// platform monogram always.
///
/// Extracted so `web_manifest_for` stays inside the ~60-line guidance, and
/// because "which icons does this app advertise" is a question worth being able
/// to answer in one place.
async fn manifest_icons_for(
    db: &DatabaseConnection,
    app: &entity::apps::Model,
    base_path: &str,
) -> Vec<crate::client::ManifestIcon> {
    let mut icons: Vec<crate::client::ManifestIcon> = Vec::new();

    // `resolve_channel` rather than a hardcoded `Published`: an app that has
    // never published resolves `NotFound` on the published channel and loses its
    // mark entirely, which is the case that actually bites.
    let channel = resolve_channel(app.published_at.is_some());

    // A manifest that fails to resolve costs the app its branding on the home
    // screen, never its installability — but it is logged, because "why is my
    // app's icon not on the home screen" is otherwise unanswerable from logs.
    match oxy_custom_apps::manifest::resolve_manifest(db, app, channel).await {
        Ok(m) => {
            if let Some(icon) = m.icon.as_deref()
                && oxy_custom_apps::manifest::safe_relative_art_path(icon)
                && let Some(mime) = crate::client::icon_mime(icon)
            {
                icons.push(crate::client::ManifestIcon {
                    src: format!("{base_path}{icon}"),
                    mime,
                    // Only claim a size for a scalable format. We never decode an
                    // author's raster, so any pixel count here would be invented
                    // — and a browser that trusts an invented size and finds
                    // something smaller drops the icon entirely.
                    //
                    // The consequence, worth naming: Blink scores candidates by
                    // declared `sizes`, so an entry with none rarely wins against
                    // the monogram below. Author artwork is effectively
                    // decorative until publish records real dimensions.
                    sizes: (mime == "image/svg+xml").then_some("any"),
                    // Not `maskable`: Android crops ~20% off each edge, and this
                    // is artwork whose safe zone we have never seen.
                    purpose: "any",
                });
            }
        }
        Err(e) => {
            tracing::warn!(
                app_id = %app.id,
                error = %e,
                "could not resolve app manifest for web-manifest icons — falling back to the monogram"
            );
        }
    }

    icons.push(crate::client::ManifestIcon {
        src: format!("{base_path}{}", crate::client::MONOGRAM_ICON_PATH),
        mime: "image/svg+xml",
        // A MEASURED claim, unlike the author's raster above: `monogram_svg`
        // emits `viewBox="0 0 512 512" width="512" height="512"`, so this is our
        // own artwork's real size. It matters because a bare `any` on an SVG is
        // exactly what Chrome's installability check has historically been
        // unreliable about — and an unusable fallback icon makes the whole
        // fallback a silent no-op for the apps it exists for.
        sizes: Some("512x512 any"),
        // The platform drew this one, inside the maskable safe zone.
        purpose: "any maskable",
    });

    icons
}

/// Serve the synthesised web app manifest — what makes an app installable.
///
/// **The scope must match the service worker's**, and computing it the same way
/// from the same input is how that is guaranteed rather than remembered. On a
/// custom-app subdomain the page sits at `/` while the bundle's assets keep the
/// `/customer-apps/<org>/<app>/` prefix; on the admin host both are under the
/// base path. A manifest whose `scope` does not cover the page is not an error
/// any browser reports — it just silently declines to install, which is the
/// worst failure available to a feature whose entire purpose is installation.
///
/// Icons resolve against the BASE path in both cases, because that is where the
/// bundle's files actually live on either surface.
///
/// The app's own icon is optional and author-declared, so it cannot be the only
/// entry: a browser will not offer installation without an icon it can fetch,
/// and most apps ship none (which is why the launcher has a monogram fallback
/// at all). The platform monogram is therefore always present as the last
/// entry, and the author's mark — resolved through the same manifest and the
/// same sanitiser every other surface uses, per `oxy-app-visual-identity` —
/// goes first when there is one.
///
/// Reading the manifest costs a query only when the app has no
/// `manifest_override`, and this route is hit on install and update checks
/// rather than on navigation, so it is not on the path `oxy-customer-apps-perf`
/// is about.
async fn web_manifest_for(
    db: &DatabaseConnection,
    runtime: &AppRuntimeConfig,
    app: &entity::apps::Model,
    on_app_host: bool,
) -> Response {
    let page_scope = if on_app_host { "/" } else { &runtime.base_path };

    let icons = manifest_icons_for(db, app, &runtime.base_path).await;

    let body = crate::client::web_manifest_json(
        &app.name,
        &short_name_for(&app.name),
        app.id,
        page_scope,
        &icons,
    );

    (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/manifest+json",
            ),
            // Short, not immutable: the manifest is derived from the app's name
            // and the surface it was requested on, and a rename that took a
            // year to reach installed devices would be worse than a re-fetch.
            //
            // `private`, not `public`, for the reason `cache_control_for`
            // already gives every other object in this namespace: it sits
            // behind the app's auth gate and names the app, so a shared cache
            // holding it could hand it to a caller who never passed that gate.
            (axum::http::header::CACHE_CONTROL, "private, max-age=300"),
        ],
        body,
    )
        .into_response()
}

/// A home-screen label, which has room for far less than a name.
///
/// Truncated on a word boundary rather than swapped for the slug: the slug is
/// not reliably shorter (`"Store Operations"` and `"store-operations"` are the
/// same length), so falling back to it traded the capitals for nothing.
fn short_name_for(name: &str) -> String {
    const MAX: usize = 12;
    if name.chars().count() <= MAX {
        return name.to_string();
    }
    let mut out = String::new();
    for word in name.split_whitespace() {
        let sep = usize::from(!out.is_empty());
        if out.chars().count() + sep + word.chars().count() > MAX {
            break;
        }
        if sep == 1 {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        // One long word, so there is no boundary to cut on.
        out = name.chars().take(MAX).collect();
    }
    out
}

/// Serve the platform monogram — the icon every app is guaranteed to have.
///
/// Under the reserved namespace and behind the app's own gate, like everything
/// else here, so it needs no separate authorization story.
fn monogram_icon_for(app: &entity::apps::Model) -> Response {
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "image/svg+xml"),
            // Same reasoning as the manifest it is named from: derived from the
            // app's name, behind the app's gate, so short and private.
            (axum::http::header::CACHE_CONTROL, "private, max-age=300"),
        ],
        crate::client::monogram_svg(&app.name),
    )
        .into_response()
}

/// Read a telemetry batch and hand it to the beacon.
///
/// The caller has already authenticated the request and confirmed this viewer
/// may open the app — the beacon deliberately makes no second authorization
/// decision, because it is dispatched from inside the gate the bundle's own
/// bytes pass through.
#[allow(clippy::too_many_arguments)]
async fn ingest_beacon(
    body: axum::body::Body,
    headers: &HeaderMap,
    app_id: Uuid,
    org_id: Uuid,
    user_id: Uuid,
    user_email: &str,
    environment: &AppEnvironment,
    record_view: impl FnOnce(Option<&str>) -> Option<String>,
) -> Response {
    let bytes = match axum::body::to_bytes(body, crate::beacon::MAX_BODY_BYTES).await {
        Ok(b) => b,
        // `to_bytes` rejects an over-limit body before the size check inside
        // `admit` ever sees it; that one is the second line, not the first.
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    crate::beacon::handle(
        app_id,
        org_id,
        user_id,
        user_email.to_string(),
        headers,
        bytes,
        environment.name(),
        record_view,
    )
    .await
}

/// Did this request arrive on a `<org>--<app>.customer-apps…` host? Read once,
/// in [`ServeContext::prepare`], into `on_app_host` (with a custom domain,
/// which is the same kind of origin) — one fact for the recorded view's
/// `source` label, the service worker's scope and the manifest's. They must not
/// be able to disagree: the label is cosmetic, but the scope decides whether a
/// custom app's worker may claim the admin origin.
fn on_custom_app_subdomain(headers: &HeaderMap) -> bool {
    oxy_app_core::custom_apps_host_dispatch::parse_subdomain(
        headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    )
    .is_some()
}

#[cfg(test)]
mod reserved_tests {
    use super::*;

    /// The monogram must be classified, not fall through to the bundle — the
    /// whole point of the reserved namespace.
    #[test]
    fn monogram_is_a_classified_reserved_path() {
        assert!(matches!(
            classify_reserved("icon.svg", &axum::http::Method::GET),
            Reserved::MonogramIcon
        ));
        assert!(matches!(
            classify_reserved("icon.svg", &axum::http::Method::POST),
            Reserved::Unknown
        ));
    }

    /// The fallback exists to fit a home-screen label. Falling back to the slug
    /// did not: `"Store Operations"` and `"store-operations"` are both 16.
    #[test]
    fn short_name_actually_shortens() {
        assert_eq!(short_name_for("Store Ops"), "Store Ops");
        assert_eq!(short_name_for("Store Operations"), "Store");
        assert_eq!(short_name_for("Poke House Store Ops"), "Poke House");
        // One long word has no boundary to cut on, so cut anyway rather than
        // returning something that overflows.
        assert_eq!(short_name_for("Supercalifragilistic").chars().count(), 12);
        for name in [
            "Store Operations",
            "Poke House Store Ops",
            "Supercalifragilistic",
        ] {
            assert!(
                short_name_for(name).chars().count() <= 12,
                "{name} produced a label that is still too long"
            );
        }
    }

    #[test]
    fn recognises_the_reserved_namespace_and_nothing_adjacent() {
        assert_eq!(reserved_platform_path("__oxy/sw.js"), Some("sw.js"));
        assert_eq!(reserved_platform_path("/__oxy/beacon"), Some("beacon"));
        // Case-insensitive prefix, case-preserving tail.
        assert_eq!(reserved_platform_path("__OXY/sw.js"), Some("sw.js"));
        assert_eq!(reserved_platform_path("__oxy/"), Some(""));

        // An app's own files must never be captured by the prefix.
        assert_eq!(reserved_platform_path("assets/__oxy/x.js"), None);
        assert_eq!(reserved_platform_path("__oxygen/x.js"), None);
        assert_eq!(reserved_platform_path("index.html"), None);
        assert_eq!(reserved_platform_path(""), None);
        // Shorter than the prefix — the split must not panic.
        assert_eq!(reserved_platform_path("__ox"), None);
    }

    use axum::http::Method;

    #[test]
    fn classifies_the_platform_endpoints_by_name_and_method() {
        assert_eq!(
            classify_reserved("sw.js", &Method::GET),
            Reserved::ServiceWorker
        );
        assert_eq!(classify_reserved("beacon", &Method::POST), Reserved::Beacon);
        assert_eq!(
            classify_reserved("asset-manifest.json", &Method::GET),
            Reserved::BuildObject
        );
    }

    /// The live-build probe is a GET-only platform endpoint. A POST is an
    /// unknown resource like any other wrong verb, never a fall-through.
    #[test]
    fn the_live_build_probe_is_get_only() {
        assert_eq!(
            classify_reserved("build", &Method::GET),
            Reserved::LiveBuild
        );
        assert_eq!(classify_reserved("build", &Method::POST), Reserved::Unknown);
        // Not a prefix match: an app path that merely starts with the name is
        // not the probe.
        assert_eq!(classify_reserved("builds", &Method::GET), Reserved::Unknown);
    }

    /// The manifest is written into the build prefix at publish, so it must
    /// reach the ordinary dispatch rather than being answered here. Getting this
    /// wrong 404s the document the service worker's whole precache depends on —
    /// and does it silently, because a worker with no manifest simply installs
    /// and caches nothing.
    #[test]
    fn the_asset_manifest_falls_through_to_the_build() {
        assert_eq!(
            classify_reserved("asset-manifest.json", &Method::GET),
            Reserved::BuildObject,
            "the manifest is an object in the build, not a synthesised response"
        );
    }

    /// A wrong method is an unknown resource, not a 405: answering differently
    /// would advertise which verbs each platform path takes.
    #[test]
    fn a_wrong_method_or_unknown_name_is_simply_not_found() {
        assert_eq!(classify_reserved("sw.js", &Method::POST), Reserved::Unknown);
        assert_eq!(classify_reserved("beacon", &Method::GET), Reserved::Unknown);
        assert_eq!(classify_reserved("", &Method::GET), Reserved::Unknown);
        assert_eq!(
            classify_reserved("not-a-thing", &Method::GET),
            Reserved::Unknown
        );
        // An app's own file under the prefix cannot be reached either — publish
        // strips them, and this is the serve-side half of the same rule.
        assert_eq!(
            classify_reserved("secrets.json", &Method::GET),
            Reserved::Unknown
        );
    }
}

/// Test-only view of the origin's cache policy, so
/// `custom_apps_asset_manifest` can assert that the set of paths its service
/// worker serves cache-first is exactly the set this route calls `immutable`.
/// Two lists, one rule — and a divergence pins a stale chunk with no
/// server-side remedy, which is worth a test that reaches across modules.
///
/// Takes the version pin as its two raw halves so that test can also assert
/// the one `immutable` case the worker deliberately does not share.
#[cfg(test)]
pub(crate) fn cache_control_for_test_only(
    request_path: &str,
    file_path: &std::path::Path,
    requested_version: Option<&str>,
    served_build: Option<Uuid>,
) -> &'static str {
    headers::cache_control_for(
        request_path,
        file_path,
        headers::VersionPin {
            requested: requested_version,
            served: served_build,
        },
    )
}

/// The service worker's cache-first list (`oxy_custom_apps::asset_manifest`) and the
/// origin's `immutable` policy (`headers`) are two lists of one rule. They sit in
/// different crates now, so the test that pins them together sits with the policy.
#[cfg(test)]
mod origin_policy_tests {
    /// The worker serves anything in `assets` cache-first and never
    /// revalidates it. That is only sound while the origin agrees the same
    /// paths are `immutable` — see `cache_control_for`. Two lists, one rule.
    ///
    /// The worker's rule reads the PATH only (`isImmutable(relativeToBase(
    /// url.pathname))` in `sw.js` — the query is never consulted), so the
    /// equality is asserted with no version pin. The origin has exactly one
    /// `immutable` case beyond it: a file whose `?v=` names the served build.
    /// That extension is one-directional and safe — the worker sends such a
    /// request to the network, where the HTTP cache honours the origin's answer
    /// — and it must stay out of the worker's list, which the last block pins.
    #[test]
    fn immutable_prefixes_agree_with_the_origin_cache_policy() {
        use super::cache_control_for_test_only as cache_control_for;
        use oxy_custom_apps::asset_manifest::is_immutable_asset_path;
        use std::path::Path;
        use uuid::Uuid;
        let served = Uuid::from_u128(7);
        let other_build = Uuid::from_u128(8).to_string();
        for p in ["assets/x.js", "_next/static/x.js"] {
            assert!(is_immutable_asset_path(p));
            assert!(
                cache_control_for(p, Path::new(p), None, None).contains("immutable"),
                "{p} is precacheable here but not immutable at the origin"
            );
            // The worker ignores the query, so no `v` may take a precacheable
            // path out of `immutable` at the origin.
            assert!(
                cache_control_for(p, Path::new(p), Some(&other_build), Some(served))
                    .contains("immutable"),
                "{p} is precacheable here but a mismatched ?v= demoted it at the origin"
            );
        }
        for p in [
            "favicon.ico",
            "manifest.webmanifest",
            "sw-custom.js",
            "icon.svg",
        ] {
            assert!(!is_immutable_asset_path(p));
            assert!(
                !cache_control_for(p, Path::new(p), None, None).contains("immutable"),
                "{p} is immutable at the origin but not precacheable here"
            );
        }
        // The origin-only case: a launcher image pinned to the served build.
        // Immutable at the origin, and NOT claimed by the worker's path rule.
        let v = served.to_string();
        assert!(
            cache_control_for("icon.svg", Path::new("icon.svg"), Some(&v), Some(served))
                .contains("immutable")
        );
        assert!(
            !is_immutable_asset_path("icon.svg"),
            "the worker must not cache-first a root file: it cannot see the ?v= pin"
        );
    }
}
