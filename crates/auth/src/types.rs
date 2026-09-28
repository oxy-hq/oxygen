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
}

impl AuthenticatedUser {
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
