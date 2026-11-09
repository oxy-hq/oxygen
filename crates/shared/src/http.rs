//! The process's one outbound HTTP client.
//!
//! Every `reqwest::Client` is its own connection pool, TLS session cache,
//! timeout policy and user agent. Before this module the workspace built one
//! at 43 sites in 32 files — many of them inside a request handler, so a TLS handshake
//! per call and no pool at all. Now there are two entry points:
//!
//! - [`client`] — the shared client, built once, for the common call: a short
//!   JSON or form exchange with an API (OAuth token exchange, a health probe,
//!   a webhook). It carries an overall [`REQUEST_TIMEOUT`].
//! - [`builder`] — the same defaults on a fresh `ClientBuilder`, for a caller
//!   that genuinely needs its own client: a different timeout, default headers,
//!   a redirect policy, a cookie jar, a DNS resolver. The caller finishes the
//!   build. **It carries no overall timeout**, because the callers that need
//!   their own client are mostly the ones that stream (LLM responses, the IDE
//!   proxy, a ClickHouse query, a GitHub archive download) and an overall
//!   timeout would cut a legitimately long response; set `.timeout()` or
//!   `.read_timeout()` yourself, and say why.
//!
//! What the defaults deliberately leave alone:
//!
//! - **No `https_only`.** The IDE proxy and the local Airhouse and ClickHouse
//!   health probes (`oxy start`) are plain `http://` on loopback, and a
//!   customer's ClickHouse URL may be too. SSRF defence is per site
//!   (`ctx.fetch`'s resolver, web push's `check_endpoint`), not a client
//!   default.
//! - **Redirects stay at reqwest's default (follow up to 10).** A site whose
//!   target URL is caller-supplied disables them itself, next to the reason.
//! - **Proxy and root certificates** come from the environment, as before.
//!
//! The guard test in `http_tests.rs` fails the build on a `reqwest::Client`
//! constructed anywhere else outside test code, unless the site is listed in
//! its `ALLOWED` backlog with a reason. The map of every duplicated
//! infrastructure client (HTTP, ClickHouse, Postgres, S3, LLM) and the order
//! to converge them: `internal-docs/infrastructure-clients.md`.

use std::sync::OnceLock;
use std::time::Duration;

/// Time allowed to open a TCP connection and finish the TLS handshake.
/// The tightest site before this module used 3s (the IDE proxy, loopback);
/// most used none, which means the OS default of ~2 minutes.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Overall deadline for one request on [`client`], connect to last body byte.
/// Generous for an API exchange; a site that had its own shorter deadline
/// keeps it per request (`RequestBuilder::timeout` overrides this one).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an idle pooled connection is kept. reqwest's own default; stated
/// so a change is one line here rather than a surprise.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// `oxy/<workspace version>`. reqwest sends no `User-Agent` by default, and
/// GitHub and Overpass refuse a request without one. A site with its own
/// product string sets it on the builder; a request header always wins over
/// this default.
pub const USER_AGENT: &str = concat!("oxy/", env!("CARGO_PKG_VERSION"));

/// A `ClientBuilder` carrying the process defaults — user agent, connect
/// timeout, pool idle timeout — and **no overall request timeout**. For the
/// caller that needs a client of its own; see the module docs for when.
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
}

/// The shared client: [`builder`] plus [`REQUEST_TIMEOUT`], built once per
/// process. `reqwest::Client` is an `Arc` inside, so the clone is free and
/// every caller shares one pool.
pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                // Fails only if the TLS backend cannot initialise, which
                // `reqwest::Client::new()` would also panic on.
                .expect("the shared reqwest client is always constructible")
        })
        .clone()
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
