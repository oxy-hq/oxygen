//! Service tokens: platform credentials configured entirely as code.
//!
//! `OXY_SERVICE_TOKENS` is a JSON array of `{"name", "role", "sha256"}`, one per
//! service (ferret, the external monitor, is the first). The deployment holds
//! the raw token (`oxysvc_<48 alnum>`) and oxy holds only its SHA-256, so the
//! credential exists without an account, a grant, a sign-in or a mint: the
//! infrastructure generates the token and writes both halves.
//!
//! A request presenting an `oxysvc_` token in `X-API-Key` (or as a bearer)
//! authenticates as the principal `service:<name>` when its hash matches an
//! entry (compared in constant time, against every entry), and is refused 401
//! when none does — like every prefixed token, it never falls through to a
//! cookie or a legacy key. A value without the prefix is untouched.
//!
//! **Standing comes from the entry, never from `app_admins`.** The principal
//! holds its role's capabilities over every org, minus
//! `manage_platform_grants`: a service can be staff at the highest grantable
//! level and still cannot issue, re-role or revoke anyone's standing. It is
//! never the Global Owner, so owner-only surfaces stay owner-only.
//!
//! **A `users` row backs it**, created at boot by [`ensure_principals`]: a
//! deterministic id from the name, the name `service:<name>`, no email and no
//! login path — the shape service-account users already have. Audit rows,
//! function invocations and every other write that names a user need a real
//! id; with no email it is in no staff audience and no grant list. Deactivating
//! that row stops the token (admission refuses an inactive principal) until
//! someone reactivates it; boot does not.
//!
//! Unset or empty, the feature is off and every `oxysvc_` token is refused. A
//! malformed value fails startup rather than being ignored: the variable is read
//! and checked by `oxy_app_core::config::BootConfig`, which [`parse`]s it, and
//! `oxy serve` [`install`]s the result.

use std::sync::OnceLock;

use entity::users::{self, UserStatus};
use oxy_authz::{Cap, PlatformRole, PlatformStanding, Scope};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, DatabaseConnection, DbErr, EntityTrait};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::credential::{CredentialContext, StoredKind};

/// What every service token starts with.
pub const PREFIX: &str = "oxysvc_";

/// Names a service's deterministic ids, so the same name is the same principal
/// on every pod and across restarts.
const NAMESPACE: Uuid = Uuid::from_u128(0x6f78_7973_7663_0000_8000_0000_0000_0001);

/// Longest service name.
const NAME_MAX: usize = 40;

/// One configured service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceToken {
    pub name: String,
    pub role: PlatformRole,
    sha256: [u8; 32],
}

/// One entry as `OXY_SERVICE_TOKENS` spells it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    role: String,
    sha256: String,
}

impl ServiceToken {
    /// An entry for the token whose SHA-256 is `sha256_hex`, checked as
    /// [`parse`] checks one.
    pub fn new(name: &str, role: &str, sha256_hex: &str) -> Result<Self, String> {
        if name.is_empty()
            || name.len() > NAME_MAX
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!(
                "service name {name:?} must be 1-{NAME_MAX} of a-z, 0-9 and '-'"
            ));
        }
        let role = PlatformRole::from_str(role)
            .ok_or_else(|| format!("service {name}: {role:?} is not a platform role"))?;
        let is_lower_hex = sha256_hex.len() == 64
            && sha256_hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !is_lower_hex {
            return Err(format!(
                "service {name}: sha256 must be 64 lowercase hex characters"
            ));
        }
        let mut sha256 = [0u8; 32];
        hex::decode_to_slice(sha256_hex, &mut sha256)
            .map_err(|e| format!("service {name}: sha256: {e}"))?;
        Ok(Self {
            name: name.to_string(),
            role,
            sha256,
        })
    }

    /// The principal's name everywhere it is shown: audit rows, traces, the
    /// users row.
    pub fn principal(&self) -> String {
        format!("service:{}", self.name)
    }

    /// The backing `users` row's id.
    pub fn user_id(&self) -> Uuid {
        Uuid::new_v5(&NAMESPACE, format!("user:{}", self.name).as_bytes())
    }

    /// The credential's id: what usage, traces and the request's
    /// `CredentialContext` carry. No `api_tokens` row has it.
    pub fn token_id(&self) -> Uuid {
        Uuid::new_v5(&NAMESPACE, format!("token:{}", self.name).as_bytes())
    }

    /// The standing the principal holds: its role's capabilities over every
    /// org, without `manage_platform_grants`.
    pub fn standing(&self) -> PlatformStanding {
        PlatformStanding {
            role: self.role,
            caps: self
                .role
                .caps()
                .into_iter()
                .filter(|cap| *cap != Cap::ManagePlatformGrants)
                .collect(),
            scope: Scope::All,
        }
    }

    /// The request marker for this service: all-access, carrying platform
    /// standing and no partner standing, never expiring.
    pub fn credential(&self) -> CredentialContext {
        CredentialContext {
            token_id: self.token_id(),
            kind: StoredKind::Service,
            principal_user_id: self.user_id(),
            all_access: true,
            platform: true,
            partner: false,
            name: self.principal(),
            display_prefix: PREFIX.to_string(),
            legacy_api_key_id: None,
            grants: Vec::new(),
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
            blocked_orgs: Vec::new(),
            service_account: None,
            expires_at: None,
        }
    }
}

/// Parse the service-token list (`OXY_SERVICE_TOKENS`). Blank is no services.
/// Anything that is not a JSON array of well-formed entries with distinct names
/// and hashes is an error, with the reason; the value itself is never echoed.
pub fn parse(raw: &str) -> Result<Vec<ServiceToken>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let entries: Vec<Entry> = serde_json::from_str(raw)
        .map_err(|e| format!("not a JSON list of {{name, role, sha256}} entries: {e}"))?;
    let mut tokens: Vec<ServiceToken> = Vec::with_capacity(entries.len());
    for entry in entries {
        let token = ServiceToken::new(&entry.name, &entry.role, &entry.sha256)?;
        if tokens.iter().any(|t| t.name == token.name) {
            return Err(format!("service {} is named twice", token.name));
        }
        if tokens.iter().any(|t| t.sha256 == token.sha256) {
            return Err("two services have the same hash".to_string());
        }
        tokens.push(token);
    }
    Ok(tokens)
}

fn registry_cell() -> &'static OnceLock<Vec<ServiceToken>> {
    static REGISTRY: OnceLock<Vec<ServiceToken>> = OnceLock::new();
    &REGISTRY
}

fn registry() -> &'static [ServiceToken] {
    registry_cell().get().map(Vec::as_slice).unwrap_or(&[])
}

/// Make `tokens` the process's services. Once per process: installing the same
/// set again is a no-op, a different one is an error.
pub fn install(tokens: Vec<ServiceToken>) -> Result<(), String> {
    let installed = registry_cell().get_or_init(|| tokens.clone());
    if *installed == tokens {
        Ok(())
    } else {
        Err("service tokens are already installed with a different set".to_string())
    }
}

/// The service `token` is, when its hash matches an entry. Every entry is
/// compared, in constant time, whatever matched first.
pub fn authenticate(token: &str) -> Option<&'static ServiceToken> {
    if !token.starts_with(PREFIX) {
        return None;
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let mut found = None;
    for entry in registry() {
        if same_digest(&digest, &entry.sha256) {
            found = Some(entry);
        }
    }
    found
}

/// The standing of the service whose credential id is `token_id`.
pub fn standing_of(token_id: Uuid) -> Option<PlatformStanding> {
    registry()
        .iter()
        .find(|t| t.token_id() == token_id)
        .map(ServiceToken::standing)
}

/// Equal digests, in time independent of where they differ.
fn same_digest(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Create the backing `users` row of every installed service that has none.
/// Idempotent; an existing row is left as it is, a deactivated one included.
pub async fn ensure_principals(db: &DatabaseConnection) -> Result<(), DbErr> {
    for token in registry() {
        let row = users::ActiveModel {
            id: ActiveValue::Set(token.user_id()),
            email: ActiveValue::Set(None),
            name: ActiveValue::Set(token.principal()),
            picture: ActiveValue::Set(None),
            email_verified: ActiveValue::Set(false),
            magic_link_token: ActiveValue::Set(None),
            magic_link_token_expires_at: ActiveValue::Set(None),
            status: ActiveValue::Set(UserStatus::Active),
            created_at: ActiveValue::NotSet,
            last_login_at: ActiveValue::NotSet,
        };
        users::Entity::insert(row)
            .on_conflict(
                OnConflict::column(users::Column::Id)
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(db)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
