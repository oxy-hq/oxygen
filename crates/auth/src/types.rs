use entity::users::{self, UserStatus};

// Simple identity structure for email-based identity linking
#[derive(Debug, Clone)]
pub struct Identity {
    /// The user this credential names, when the credential carries an id.
    ///
    /// A session JWT always has one — `Claims.sub` has been the user id since
    /// tokens were introduced — and it is the ONLY identifier that works for a
    /// frontline worker, whose `users.email` is NULL. Resolution prefers this
    /// and falls back to [`Self::email`], which keeps every token minted before
    /// this field existed working unchanged.
    ///
    /// `None` for a provider identity (Google, Okta, magic link): at that point
    /// the address is all we have, and collapsing it onto a row is exactly what
    /// `get_or_create_user` is for.
    pub user_id: Option<uuid::Uuid>,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub id: uuid::Uuid,
    /// `None` for a frontline worker enrolled without a mailbox. Deliberately
    /// not defaulted to `""`: an empty string is indistinguishable from an
    /// address to SES, to Slack matching and to an invitation lookup, which is
    /// the failure mode `internal-docs/frontline-identity.md` exists to avoid.
    pub email: Option<String>,
    pub name: String,
    pub picture: Option<String>,
    pub status: UserStatus,
    /// The API key or token **this request** authenticated with; `None` for a
    /// browser session, and for a user loaded outside a request.
    ///
    /// It rides with the user because authorization is asked of the user in
    /// some sixty places that hold nothing else, and a token that narrows its
    /// bearer has to reach every one of them (API-tokens design §4.4). The
    /// auth entry points set it; `CredentialContext` is also attached as its
    /// own request extension, from the same value.
    ///
    /// Never copy this onto a user row loaded for someone else. A user built
    /// from the database (`From<users::Model>`) carries none — which reads as
    /// a session, so re-loading the *requester* that way inside a request
    /// would drop their token's narrowing. Take the request's own user.
    pub credential: Option<crate::token::CredentialContext>,
}

impl AuthenticatedUser {
    /// This user, as authenticated by `credential`.
    pub fn with_credential(mut self, credential: Option<crate::token::CredentialContext>) -> Self {
        self.credential = credential;
        self
    }

    /// Whether this request acts as an org's **service account** rather than
    /// as a person (API-tokens design §3.3).
    pub fn is_service_account(&self) -> bool {
        self.credential
            .as_ref()
            .is_some_and(|c| c.is_service_account())
    }

    /// The refusal of a surface that acts *as the caller* — one that hands
    /// the caller a credential of their own, say. A service account has no
    /// person behind it to hold one, so such a surface refuses it by default
    /// (design §3.3) until a consumer needs otherwise. `Err` is the status to
    /// answer; a person passes.
    pub fn refuse_service_account(&self) -> Result<(), axum::http::StatusCode> {
        if self.is_service_account() {
            tracing::warn!(
                account = %self.id,
                "service account refused on a surface that acts as the caller"
            );
            return Err(axum::http::StatusCode::FORBIDDEN);
        }
        Ok(())
    }

    /// Whether the credential this request arrived with reaches `org_id` at
    /// all. `false` when an API token holds no grant there, or the org ended
    /// the token's reach — its revoke-grant, or a token policy the token
    /// breaks (API-tokens design §5). Always `true` for a browser session and
    /// for a legacy key, which nothing narrows (§3.5).
    ///
    /// For the routes that name no org in their path and find one in what they
    /// act on: a channel, a task, a `workspace_id` query. Under
    /// `/orgs/{org_id}` and `/{workspace_id}` the middlewares already decide.
    pub fn reaches_org(&self, org_id: uuid::Uuid) -> bool {
        self.credential
            .as_ref()
            .and_then(|c| c.reach())
            .is_none_or(|reach| reach.touches_org(org_id))
    }

    /// The refusal of anything in an org the credential does not reach
    /// ([`Self::reaches_org`]): **404**, the answer everything outside a
    /// token's reach gets, so the token cannot tell it from a missing row.
    pub fn require_org_reach(&self, org_id: uuid::Uuid) -> Result<(), axum::http::StatusCode> {
        if self.reaches_org(org_id) {
            return Ok(());
        }
        tracing::warn!(
            user = %self.id,
            org = %org_id,
            "token asked for an org outside its reach — 404"
        );
        Err(axum::http::StatusCode::NOT_FOUND)
    }

    /// The orgs that ended this credential's reach into them, for an answer
    /// that spans the user's orgs to leave out (`org_id NOT IN …`). Empty for
    /// a session and for a legacy key.
    ///
    /// The list form of [`Self::reaches_org`] for an **all-access** token,
    /// which is the only token that gets as far as a route answering across
    /// orgs: a grant-bound one is refused there (`token_grant_scope`).
    pub fn blocked_orgs(&self) -> &[uuid::Uuid] {
        match &self.credential {
            Some(credential) if !credential.is_legacy() => &credential.blocked_orgs,
            _ => &[],
        }
    }

    /// A human-readable label for logs and display.
    ///
    /// The address when there is one, otherwise `name` — which is NOT NULL and
    /// is already what `get_or_create_user` populates. This is the whole reason
    /// frontline identity did NOT need a new `handle` column: the non-null
    /// human-readable identifier already existed.
    ///
    /// Never use this to *resolve* a user. It is not unique.
    pub fn label(&self) -> &str {
        self.email.as_deref().unwrap_or(&self.name)
    }

    /// A synthetic principal for an OIDC-minted, app-scoped machine publish token.
    /// It exists only to satisfy the extractor chain — the token-scope middleware
    /// confines it to the publish path, and the publish path authorizes by the
    /// token's `app_id` + client consent, never by this identity. The nil id makes
    /// it unmistakable in any log that it is not a real user.
    ///
    /// **Never persist this id.** No `users` row has it, so writing it to any
    /// column that references `users(id)` fails the FK (it 500'd every trusted
    /// publish). Record `AppPublishTokenAuth::machine_identity` instead.
    pub fn machine_publisher() -> Self {
        Self {
            id: uuid::Uuid::nil(),
            email: Some("oxy-publish-bot@oxy.internal".to_string()),
            name: "Oxy Publish (machine)".to_string(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        }
    }
}

impl From<users::Model> for AuthenticatedUser {
    fn from(user: users::Model) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            picture: user.picture,
            status: user.status,
            credential: None,
        }
    }
}

/// Request-extension marker: the request authenticated via an app publish token
/// (`oxypublish_...` bearer), not a session JWT/cookie or an API key.
///
/// App publish tokens are deliberately narrow — they authorize the customer-apps
/// admin surface only. This marker is what the scope-enforcement middleware
/// keys off to reject an app-publish-token request that targets any other route.
/// Its presence means "downstream must treat this identity as scope-limited."
#[derive(Debug, Clone)]
pub struct AppPublishTokenAuth {
    pub token_id: uuid::Uuid,
    /// Set for any **app-scoped** publish token — OIDC-minted (no human) or
    /// partner-minted (a real `created_by`, design §7). The publish path authorizes
    /// such a token strictly by this app id + the client's consent, so it is
    /// confined to that one app. `None` for an app-unscoped staff token.
    pub app_id: Option<uuid::Uuid>,
    /// Set iff this is an OIDC-minted machine token: the identity the exchange
    /// verified (the token's `name`). The request's user is then
    /// [`AuthenticatedUser::machine_publisher`], which has no `users` row, so
    /// anything recording "who did this" must write this, never that user's id.
    pub machine_identity: Option<String>,
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod types_tests;
