//! Provider descriptors for the shared OAuth 2.0 authorization-code flow.
//!
//! The QuickBooks connect flow (`integrations::quickbooks::oauth`) is, apart
//! from three constants, entirely generic: a nonce-guarded state row, a consent
//! URL, a code-for-token exchange, and a refresh token written to the workspace
//! secret manager under a caller-named var. The open-redirect guard, the CSRF
//! nonce and the popup/redirect return modes are all provider-neutral.
//!
//! Rather than clone those ~540 lines for the second provider — and then the
//! third — the provider-specific surface lives here as data.
//!
//! **Adding a provider does not mean adding a token refresher.** Rotation is
//! still app code today (`refresh-qb-token.ts` is 192 lines of it, whose
//! single-writer rule is enforced by a comment block and nothing else). This
//! module is only the *acquisition* half; see
//! `internal-docs/2026-08-18-custom-apps-vercel-parity-and-oltp.md` §5.6(1).

/// What differs between one OAuth 2.0 authorization-code provider and another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// Stable slug — appears in routes and in stored state rows.
    pub slug: &'static str,
    /// Where the user is sent to consent.
    pub authorize_url: &'static str,
    /// Where an authorization code is exchanged for tokens.
    pub token_url: &'static str,
    /// Space-separated scope string sent with the consent request.
    pub scope: &'static str,
    /// Path the provider redirects back to, absolute from the host root.
    ///
    /// Data rather than a formatted `/api/oauth/{slug}/callback`, because a
    /// callback URL is **registered with the vendor**: QuickBooks' path predates
    /// this descriptor and is registered in every customer's Intuit app, so
    /// renaming it would break every existing connection. New providers get the
    /// uniform path; QuickBooks keeps the one it shipped with, permanently.
    pub callback_path: &'static str,
    /// Query flag the FE watches for on a full-page redirect back, and the
    /// popup landing path. Data for the same reason `callback_path` is: these
    /// are a **contract with already-shipped frontend code**. QuickBooks'
    /// `qb_connected` / `/quickbooks/connected` predate this descriptor and are
    /// read by `useQuickBooksConnect`; new providers get the uniform pair.
    pub connected_flag: &'static str,
    /// Frontend route the popup lands on, or `None` when this provider has no
    /// such page. `None` means `mode: "popup"` is **refused at authorize time**
    /// — before consent — rather than stranding the user on a 404 with their
    /// token already stored.
    pub popup_path: Option<&'static str>,
    /// Whether the provider returns a tenant id worth handing back to the FE.
    /// QuickBooks' `realmId` identifies the company that was connected; Google
    /// has no equivalent, and appending an always-empty `realm_id=` to its
    /// return URL plants a QuickBooks concept in another vendor's flow.
    pub returns_realm_id: bool,
    /// Extra query parameters the provider needs on the consent URL.
    ///
    /// Google is the reason this exists: without `access_type=offline` it
    /// returns no refresh token at all, and without `prompt=consent` it returns
    /// one only on the user's *first* ever consent — so a re-connect silently
    /// yields a grant that cannot be refreshed. Intuit needs neither.
    pub authorize_extra_params: &'static [(&'static str, &'static str)],
}

impl Provider {
    /// Build the consent URL the browser is sent to.
    ///
    /// Every value is percent-encoded: `redirect_uri` carries `://` and `/`,
    /// and a scope may be a URL in its own right (Google's are), so an
    /// unencoded join would truncate the query at the first `&` or `?`.
    pub fn consent_url(&self, client_id: &str, redirect_uri: &str, nonce: &str) -> String {
        let mut url = format!(
            "{}?client_id={}&response_type=code&scope={}&redirect_uri={}&state={}",
            self.authorize_url,
            urlencoding::encode(client_id),
            urlencoding::encode(self.scope),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(nonce),
        );
        for (k, v) in self.authorize_extra_params {
            url.push('&');
            url.push_str(&urlencoding::encode(k));
            url.push('=');
            url.push_str(&urlencoding::encode(v));
        }
        url
    }

    /// Absolute callback URL for this provider on the host that served the
    /// authorize request. Must match byte-for-byte at token exchange, so it is
    /// computed once and stored on the state row.
    pub fn callback_url(&self, proto: &str, host: &str) -> String {
        format!("{proto}://{host}{}", self.callback_path)
    }
}

pub const QUICKBOOKS: Provider = Provider {
    slug: "quickbooks",
    authorize_url: "https://appcenter.intuit.com/connect/oauth2",
    token_url: "https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer",
    scope: "com.intuit.quickbooks.accounting",
    callback_path: "/api/quickbooks/oauth/callback",
    connected_flag: "qb_connected",
    popup_path: Some("/quickbooks/connected"),
    returns_realm_id: true,
    authorize_extra_params: &[],
};

/// Google Drive, scoped to files the app itself creates.
///
/// `drive.file` is Google's **non-sensitive** Drive scope, so it needs no
/// verification review — but it only ever grants access to files this client
/// created or the user explicitly opened with it. That is sufficient to write
/// and re-read our own artifacts, and NOT sufficient to read a customer's
/// existing tree. A read-broadly use case (indexing leases already sitting in
/// their Drive) needs `drive.readonly`, which IS restricted and does require
/// review — a product decision, not a constant to flip here.
pub const GOOGLE_DRIVE_FILE: Provider = Provider {
    slug: "google-drive",
    authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
    token_url: "https://oauth2.googleapis.com/token",
    scope: "https://www.googleapis.com/auth/drive.file",
    callback_path: "/api/oauth/google-drive/callback",
    connected_flag: "oxy_connected",
    // No popup landing page exists for Drive, and the shipped Connections UI
    // uses a full-page redirect. Adding one is a frontend route, not a constant.
    popup_path: None,
    returns_realm_id: false,
    authorize_extra_params: &[("access_type", "offline"), ("prompt", "consent")],
};

pub const ALL: &[Provider] = &[QUICKBOOKS, GOOGLE_DRIVE_FILE];

/// Resolve a provider by slug. `None` for anything unknown — a caller must not
/// fall back to a default, or a typo'd slug would send a user's consent to the
/// wrong vendor.
pub fn by_slug(slug: &str) -> Option<Provider> {
    ALL.iter().copied().find(|p| p.slug == slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the values the shipped QuickBooks flow uses today. If a refactor
    /// routes QuickBooks through this descriptor and one of these drifts, the
    /// symptom in production is a consent screen that 404s at Intuit.
    #[test]
    fn quickbooks_matches_the_shipped_constants() {
        assert_eq!(
            QUICKBOOKS.authorize_url,
            "https://appcenter.intuit.com/connect/oauth2"
        );
        assert_eq!(
            QUICKBOOKS.token_url,
            "https://oauth.platform.intuit.com/oauth2/v1/tokens/bearer"
        );
        assert_eq!(QUICKBOOKS.scope, "com.intuit.quickbooks.accounting");
        assert!(QUICKBOOKS.authorize_extra_params.is_empty());
        // Registered in every customer's Intuit app. Changing it breaks every
        // existing connection, and the break is invisible until someone
        // reconnects.
        assert_eq!(QUICKBOOKS.callback_path, "/api/quickbooks/oauth/callback");
        // Read by the shipped useQuickBooksConnect hook.
        assert_eq!(QUICKBOOKS.connected_flag, "qb_connected");
        assert_eq!(QUICKBOOKS.popup_path, Some("/quickbooks/connected"));
        assert!(QUICKBOOKS.returns_realm_id);
    }

    /// Google returns NO refresh token without `access_type=offline`, and only
    /// returns one on a re-consent when `prompt=consent` is present. Losing
    /// either turns a successful-looking connect into a grant that cannot be
    /// refreshed — visible only when the first access token expires an hour later.
    #[test]
    fn google_requests_an_offline_grant_and_forces_re_consent() {
        let params = GOOGLE_DRIVE_FILE.authorize_extra_params;
        assert!(params.contains(&("access_type", "offline")), "{params:?}");
        assert!(params.contains(&("prompt", "consent")), "{params:?}");
    }

    /// `drive.file` is the non-sensitive scope. Widening it to `drive` or
    /// `drive.readonly` triggers a Google verification review and changes what
    /// a customer is consenting to, so it should never happen by accident.
    #[test]
    fn google_uses_the_non_sensitive_drive_scope() {
        assert_eq!(
            GOOGLE_DRIVE_FILE.scope,
            "https://www.googleapis.com/auth/drive.file"
        );
    }

    /// Every callback path must be distinct, or two providers redeem each
    /// other's nonces at the same route.
    #[test]
    fn callback_paths_are_unique_and_rooted() {
        let mut seen = std::collections::BTreeSet::new();
        for p in ALL {
            assert!(p.callback_path.starts_with("/api/"), "{}", p.slug);
            assert!(
                seen.insert(p.callback_path),
                "duplicate: {}",
                p.callback_path
            );
        }
    }

    /// Pins the exact consent URL the shipped QuickBooks authorize handler
    /// builds today, so routing it through `consent_url` cannot change what
    /// Intuit receives.
    #[test]
    fn quickbooks_consent_url_is_unchanged_by_the_descriptor() {
        assert_eq!(
            QUICKBOOKS.consent_url(
                "abc",
                "https://app.example.com/api/quickbooks/oauth/callback",
                "n1"
            ),
            "https://appcenter.intuit.com/connect/oauth2\
             ?client_id=abc\
             &response_type=code\
             &scope=com.intuit.quickbooks.accounting\
             &redirect_uri=https%3A%2F%2Fapp.example.com%2Fapi%2Fquickbooks%2Foauth%2Fcallback\
             &state=n1"
        );
    }

    #[test]
    fn google_consent_url_carries_the_offline_params_and_encodes_its_url_scope() {
        let url = GOOGLE_DRIVE_FILE.consent_url(
            "cid",
            "https://app.example.com/api/oauth/google-drive/callback",
            "n2",
        );
        assert!(
            url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"),
            "{url}"
        );
        assert!(url.contains("&access_type=offline"), "{url}");
        assert!(url.contains("&prompt=consent"), "{url}");
        // The scope is itself a URL — unencoded it would truncate the query.
        assert!(
            url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file"),
            "{url}"
        );
    }

    #[test]
    fn callback_url_joins_without_a_double_slash() {
        assert_eq!(
            QUICKBOOKS.callback_url("https", "app.example.com"),
            "https://app.example.com/api/quickbooks/oauth/callback"
        );
    }

    /// A provider without a popup landing page must not advertise one: the
    /// token is already stored by the time the redirect happens, so a missing
    /// page is a 404 *after* a successful connect.
    #[test]
    fn a_provider_without_a_popup_page_declares_none() {
        assert_eq!(GOOGLE_DRIVE_FILE.popup_path, None);
        assert!(!GOOGLE_DRIVE_FILE.returns_realm_id);
    }

    #[test]
    fn unknown_slugs_do_not_fall_back_to_a_default() {
        assert_eq!(by_slug("quickbooks"), Some(QUICKBOOKS));
        assert_eq!(by_slug("google-drive"), Some(GOOGLE_DRIVE_FILE));
        assert_eq!(by_slug("quickbook"), None);
        assert_eq!(by_slug(""), None);
    }
}
