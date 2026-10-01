//! Credentials for a workspace preview: it reads whatever its role can read and
//! writes only its own schemas, `preview_<key>__<live schema>`
//! ([`PreviewNamespace`]).
//!
//! This is the third of the three fences that keep a preview's writes off live
//! tables (the rewrite and the verifier in [`crate::preview_sql`] are the other
//! two). A Writer mint asks Airhouse for `write_schemas`; Airhouse 0.1.49 and
//! later confine the credential to exactly those schemas (`airhouse-server`'s
//! `write_scope`) and echo the list back as sent. An older Airhouse ignores the
//! field and mints a tenant-wide Writer without saying so. So, unlike an app's
//! Writer (logged and used), a preview Writer is guarded twice:
//!
//! 1. before minting, the deployment must say it scopes Writers (`GET
//!    /admin/v1/capabilities` → `mint.write_schemas`, asked within
//!    [`PROBE_WITHIN`]; a yes is kept until point 2 proves it wrong, a "no" is
//!    asked again after [`RECHECK_UNSUPPORTED`] so an upgrade is noticed
//!    without a restart, and no answer is an error that keeps nothing);
//! 2. after minting, the echo must be exactly the scope asked for, or the
//!    credential is evicted and revoked — and an unconfined Writer also
//!    un-keeps a cached "yes" from point 1, so every later preview write
//!    short-circuits on `ScopedWritersUnsupported` instead of paying a mint +
//!    revoke round trip on Airhouse's dime for as long as the downgrade lasts.
//!
//! Either failing leaves preview writes held rather than resting on the host's
//! verifier alone, and forgets every cached preview credential: after a
//! downgrade, none minted before it is handed out again. What was already
//! handed out runs until it expires ([`super::DEFAULT_INTERNAL_TTL`], 15
//! minutes), which bounds the window in which a downgraded Airhouse can serve
//! a preview Writer it no longer confines.
//!
//! What a scoped Writer may run (Airhouse's `service-accounts.md`): DML and
//! table/view DDL on `schema.object` in scope, `CREATE SCHEMA` of a schema in
//! scope, one statement per query, bare `BEGIN`/`COMMIT`/`ROLLBACK`. It may
//! never `DROP SCHEMA`, which is why the TTL drop runs on a system Writer
//! (`SystemPurpose::PreviewDdl`).

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tracing::warn;
use uuid::Uuid;

use super::{AirhouseTokenBroker, BrokerError, BrokerSubject};
use crate::admin::{EphemeralCredential, UserRole};
use crate::preview_sql::PreviewNamespace;

/// Airhouse's cap on schemas per credential (`MAX_WRITE_SCHEMAS`).
const MAX_WRITE_SCHEMAS: usize = 64;

/// Airhouse's cap on a scoped schema name: `^[a-z_][a-z0-9_]{0,62}$`.
const MAX_SCHEMA_NAME: usize = 63;

/// How long a "this Airhouse does not scope Writers" answer is trusted.
pub(super) const RECHECK_UNSUPPORTED: Duration = Duration::from_secs(300);

/// How long the capabilities probe may take. No answer is an error, not a
/// "no", and nothing is kept.
const PROBE_WITHIN: Duration = Duration::from_secs(10);

/// Whether this deployment's Airhouse scopes Writers, and when it was asked.
/// The lock is held only to read or record the answer, never across the
/// probe, so a slow Airhouse holds up no one else's mint; mints that find
/// nothing kept may each ask.
pub(super) struct ScopedWriters {
    pub(super) known: Mutex<Option<(bool, Instant)>>,
    /// How long the probe may take: [`PROBE_WITHIN`].
    pub(super) probe_within: Duration,
}

impl Default for ScopedWriters {
    fn default() -> Self {
        Self {
            known: Mutex::new(None),
            probe_within: PROBE_WITHIN,
        }
    }
}

impl ScopedWriters {
    /// The answer kept, while it is trusted: a yes for good, a no for
    /// [`RECHECK_UNSUPPORTED`].
    fn kept(&self) -> Option<bool> {
        match *self.known.lock().unwrap_or_else(PoisonError::into_inner) {
            Some((true, _)) => Some(true),
            Some((false, at)) if at.elapsed() < RECHECK_UNSUPPORTED => Some(false),
            _ => None,
        }
    }

    fn keep(&self, supported: bool) {
        *self.known.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((supported, Instant::now()));
    }
}

impl AirhouseTokenBroker {
    /// Mint a credential for preview `ns` of `workspace_id`.
    ///
    /// A `Reader` takes no schemas. A `Writer` takes the preview schemas it
    /// will write — each one `ns` owns, lowercase, as Airhouse stores it — and
    /// comes back confined to exactly those, or not at all
    /// ([`BrokerError::ScopedWritersUnsupported`],
    /// [`BrokerError::UnscopedWriter`]). An `Admin` is refused. The scope is
    /// sorted and deduplicated here, so one set is one credential whatever
    /// order it was named in.
    pub async fn mint_for_preview(
        &self,
        workspace_id: Uuid,
        ns: &PreviewNamespace,
        schemas: &[String],
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let mut scope = schemas.to_vec();
        scope.sort();
        scope.dedup();
        let subject = BrokerSubject::Preview {
            workspace_id,
            preview_key: ns.key().to_string(),
            schemas: scope,
        };
        self.mint(workspace_id, subject, role, ttl).await
    }

    /// Every mint of a [`BrokerSubject::Preview`], whichever entry point
    /// built it: the request is checked, a Writer needs a deployment that
    /// scopes Writers, and the credential (cached or fresh) must come back
    /// scoped as asked.
    pub(super) async fn mint_preview(
        &self,
        workspace_id: Uuid,
        subject: &BrokerSubject,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let BrokerSubject::Preview {
            preview_key,
            schemas,
            ..
        } = subject
        else {
            return Err(BrokerError::PreviewScope("not a preview subject".into()));
        };
        check_request(preview_key, schemas, role)?;
        if role == UserRole::Writer {
            self.require_scoped_writers().await?;
        }
        let cred = self
            .mint_or_cached(workspace_id, subject, role, ttl)
            .await?;
        if let Err(refused) = confirm_scope(schemas, role, &cred) {
            self.discard(workspace_id, subject, role, &cred).await;
            if matches!(refused, BrokerError::UnscopedWriter { .. }) {
                // The deployment just proved a cached "yes" wrong: un-keep it
                // so the next preview write short-circuits on
                // `ScopedWritersUnsupported` (re-probed after
                // `RECHECK_UNSUPPORTED`) instead of paying a mint + revoke
                // round trip on Airhouse's dime for as long as the downgrade
                // lasts.
                self.scoped_writers.keep(false);
                self.forget_preview_credentials(
                    "Airhouse minted a preview Writer it did not confine",
                )
                .await;
            }
            return Err(refused);
        }
        Ok(cred)
    }

    /// Whether this deployment's Airhouse confines a Writer to named schemas
    /// (0.1.49 and later): the answer every preview Writer mint asks first,
    /// kept and re-asked the same way. A host asks it before it does any
    /// preview write work, so an Airhouse that cannot confine one leaves the
    /// preview's writes held rather than half-prepared. `Err` when Airhouse
    /// did not answer ([`BrokerError::CapabilitiesUnanswered`]) or could not
    /// be asked.
    pub async fn scopes_preview_writers(&self) -> Result<bool, BrokerError> {
        match self.scoped_writers.kept() {
            Some(kept) => Ok(kept),
            None => self.probe_scoped_writers().await,
        }
    }

    /// `Ok` when this deployment's Airhouse scopes Writers.
    async fn require_scoped_writers(&self) -> Result<(), BrokerError> {
        if self.scopes_preview_writers().await? {
            Ok(())
        } else {
            Err(BrokerError::ScopedWritersUnsupported)
        }
    }

    /// Ask the deployment, within [`ScopedWriters::probe_within`], and keep
    /// the answer. A "no" also forgets every cached preview credential.
    async fn probe_scoped_writers(&self) -> Result<bool, BrokerError> {
        let within = self.scoped_writers.probe_within;
        let capabilities = tokio::time::timeout(within, self.client.capabilities())
            .await
            .map_err(|_| BrokerError::CapabilitiesUnanswered(within))??;
        let supported = capabilities.mint_write_schemas;
        self.scoped_writers.keep(supported);
        if !supported {
            self.forget_preview_credentials("this Airhouse does not confine Writers")
                .await;
        }
        Ok(supported)
    }

    /// Drop every cached preview credential, of any preview and role: on a
    /// sign that Airhouse no longer confines Writers, none minted before it is
    /// handed out again. Credentials already handed out, and connections open
    /// on them, run until they expire.
    async fn forget_preview_credentials(&self, why: &str) {
        let mut cache = self.cache.write().await;
        let before = cache.len();
        cache.retain(|(_, subject, _), _| !is_preview_subject(subject));
        let forgotten = before - cache.len();
        if forgotten > 0 {
            warn!(
                forgotten,
                "{why}: forgot the cached preview airhouse credentials"
            );
        }
    }

    /// Forget a credential that must not be used, and revoke it on Airhouse
    /// (best effort: a failed revoke is logged, and the credential expires on
    /// its own).
    async fn discard(
        &self,
        workspace_id: Uuid,
        subject: &BrokerSubject,
        role: UserRole,
        cred: &EphemeralCredential,
    ) {
        self.evict(workspace_id, subject, role).await;
        if let Err(e) = self.revoke_user_token(workspace_id, &cred.username).await {
            warn!(
                workspace_id = %workspace_id,
                subject = %subject.audit_subject(),
                username = %cred.username,
                "could not revoke a refused airhouse credential; it expires at {}: {e}",
                cred.expires_at
            );
        }
    }
}

/// Refuse, before anything is minted, a preview credential the preview may
/// not have. The scope must be sorted and distinct ([`mint_for_preview`]
/// makes it so): its cache entry is keyed by it as given, and the echo is
/// compared with it as given, so a reordered scope would otherwise find a
/// valid shared credential, fail the comparison, and evict and revoke it.
///
/// [`mint_for_preview`]: AirhouseTokenBroker::mint_for_preview
fn check_request(preview_key: &str, schemas: &[String], role: UserRole) -> Result<(), BrokerError> {
    let refuse = |why: String| Err(BrokerError::PreviewScope(why));
    let ns = PreviewNamespace::from_key(preview_key)
        .map_err(|e| BrokerError::PreviewScope(e.to_string()))?;
    match role {
        UserRole::Admin => refuse(
            "a preview never mints an Admin; it writes only through a Writer confined to its \
             own schemas"
                .into(),
        ),
        UserRole::Reader if schemas.is_empty() => Ok(()),
        UserRole::Reader => refuse(format!(
            "a Reader carries no write scope, but {schemas:?} was asked for"
        )),
        UserRole::Writer if schemas.is_empty() => refuse(
            "a preview Writer must name the schemas it writes; without them Airhouse mints a \
             tenant-wide Writer"
                .into(),
        ),
        UserRole::Writer if schemas.len() > MAX_WRITE_SCHEMAS => refuse(format!(
            "{} schemas asked for; Airhouse scopes at most {MAX_WRITE_SCHEMAS}",
            schemas.len()
        )),
        UserRole::Writer => {
            if let Some(bad) = schemas
                .iter()
                .find(|s| !(is_scope_name(s) && ns.owns_schema(s)))
            {
                return refuse(format!(
                    "{bad:?} is not one of preview {preview_key}'s schemas \
                     (preview_{preview_key}__<live schema>, lowercase, at most \
                     {MAX_SCHEMA_NAME} characters)"
                ));
            }
            if !schemas.windows(2).all(|pair| pair[0] < pair[1]) {
                return refuse(format!(
                    "{schemas:?} must be sorted and name each schema once"
                ));
            }
            Ok(())
        }
    }
}

/// Refuse a credential Airhouse did not scope as asked: a Writer must echo
/// exactly `schemas`, and a Reader must come back a Reader.
fn confirm_scope(
    schemas: &[String],
    role: UserRole,
    cred: &EphemeralCredential,
) -> Result<(), BrokerError> {
    match role {
        UserRole::Writer if cred.write_schemas.as_deref() == Some(schemas) => Ok(()),
        UserRole::Writer => Err(BrokerError::UnscopedWriter {
            asked: schemas.to_vec(),
            echoed: cred.write_schemas.clone(),
        }),
        UserRole::Reader if cred.role.eq_ignore_ascii_case(UserRole::Reader.as_str()) => Ok(()),
        _ => Err(BrokerError::PreviewScope(format!(
            "asked Airhouse for a {} and got a {:?}",
            role.as_str(),
            cred.role
        ))),
    }
}

/// Whether a cache entry's subject is a [`BrokerSubject::Preview`]'s,
/// `system:workspace:<id>:preview:<key>[|<scope>]`. The live-table reads of
/// `SystemPurpose::Preview` end at `:preview` and are not one.
fn is_preview_subject(subject: &str) -> bool {
    subject
        .strip_prefix("system:workspace:")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(_, purpose)| purpose.starts_with("preview:"))
}

/// `^[a-z_][a-z0-9_]{0,62}$`: what Airhouse accepts as a scoped schema.
fn is_scope_name(name: &str) -> bool {
    let Some((&first, rest)) = name.as_bytes().split_first() else {
        return false;
    };
    name.len() <= MAX_SCHEMA_NAME
        && (first.is_ascii_lowercase() || first == b'_')
        && rest
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod capability_tests;
