//! The audience this deployment's exchange requires (API-tokens design §3.4).
//!
//! Per deployment, so a GitHub OIDC token asked for one deployment cannot be
//! replayed at another: spent `jti`s are recorded per database, and an
//! audience every deployment shared would leave nothing else in the way. Why,
//! and the shape of the value, are in `oxy_auth::github_oidc::audience`.
//!
//! **It is not served.** A client works the audience out from the URL it is
//! talking to (`oxy:<host>`), and this deployment accepts the one derived from
//! the URL it calls itself by — its configured public base URL, the same
//! source the token mails link through. The two agree exactly when the client
//! was pointed at this deployment's own address. A route that told a client
//! what to ask for would let a deployment name another deployment's audience
//! and be handed a token that one accepts; so there is none, and the only
//! place the value appears on the wire is a `wrong_audience` refusal, which
//! says what this deployment takes to a caller that asked for something else.

use oxy_app_core::custom_apps_host_dispatch::admin_base_url;
use oxy_auth::github_oidc::deployment_audience;

/// The audience the exchange accepts, and the only one: `oxy:<host>` of the
/// deployment's public base URL, or plain `oxy` where none is configured —
/// which no client asks for, so such a deployment takes no GitHub sign-in.
pub fn audience() -> String {
    deployment_audience(admin_base_url().as_deref())
}
