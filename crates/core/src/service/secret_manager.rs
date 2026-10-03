use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, Generate, KeyInit, Nonce as AeadNonce},
};
use base64::{Engine as _, engine::general_purpose};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, EntityTrait, QueryFilter,
    QueryOrder, Set, Statement, TransactionTrait,
};
use secrecy::{ExposeSecret, SecretString};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use entity::secrets::{self, ActiveModel as SecretActiveModel, Entity as Secret};
use oxy_shared::errors::OxyError;

use crate::{database::client::establish_connection, utils::get_encryption_key};

/// A managed secret that holds a reference to a secret key variable.
///
/// This type can be deserialized directly from a string in YAML/JSON configs:
/// ```yaml
/// secret: AWS_S3_SECRET
/// ```
///
/// The actual secret value is retrieved via the `expose` method which
/// looks up the secret from either `SecretManagerService` (database) or
/// `SecretsManager` (supports both env vars and database).
#[derive(Clone)]
pub struct ManagedSecret {
    key_var: String,
}

impl ManagedSecret {
    /// Create a new ManagedSecret with the given key variable name.
    pub fn new(key_var: impl Into<String>) -> Self {
        Self {
            key_var: key_var.into(),
        }
    }

    /// Get the key variable name.
    pub fn key_var(&self) -> &str {
        &self.key_var
    }

    /// Expose the secret value by looking it up from the SecretManagerService.
    ///
    /// Returns the secret wrapped in a `SecretString` for safe handling.
    pub async fn expose(
        &self,
        secret_manager: &SecretManagerService,
    ) -> Result<SecretString, OxyError> {
        secret_manager
            .get_secret(&self.key_var)
            .await
            .map(SecretString::from)
            .ok_or_else(|| OxyError::SecretManager(format!("Secret '{}' not found", self.key_var)))
    }

    /// Expose the secret value as a plain string using SecretManagerService.
    ///
    /// Use this when you need the raw string value. Prefer `expose()` when possible
    /// to keep the secret wrapped in `SecretString`.
    pub async fn expose_str(
        &self,
        secret_manager: &SecretManagerService,
    ) -> Result<String, OxyError> {
        let secret = self.expose(secret_manager).await?;
        Ok(secret.expose_secret().to_string())
    }

    /// Expose the secret value using the SecretsManager adapter.
    ///
    /// This is the preferred method as it supports both environment variables
    /// and database-backed secrets.
    pub async fn expose_with_adapter(
        &self,
        secrets_manager: &crate::adapters::secrets::SecretsManager,
    ) -> Result<SecretString, OxyError> {
        secrets_manager
            .resolve_secret(&self.key_var)
            .await?
            .map(SecretString::from)
            .ok_or_else(|| OxyError::SecretManager(format!("Secret '{}' not found", self.key_var)))
    }

    /// Expose the secret value as a plain string using the SecretsManager adapter.
    pub async fn expose_str_with_adapter(
        &self,
        secrets_manager: &crate::adapters::secrets::SecretsManager,
    ) -> Result<String, OxyError> {
        secrets_manager
            .resolve_secret(&self.key_var)
            .await?
            .ok_or_else(|| OxyError::SecretManager(format!("Secret '{}' not found", self.key_var)))
    }
}

impl fmt::Debug for ManagedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Don't expose the key_var in debug output for security
        f.debug_struct("ManagedSecret")
            .field("key_var", &"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for ManagedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ManagedSecret({})", self.key_var)
    }
}

// Deserialize directly from a string
impl<'de> serde::Deserialize<'de> for ManagedSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let key_var = String::deserialize(deserializer)?;
        Ok(ManagedSecret { key_var })
    }
}

// Serialize as a string
impl serde::Serialize for ManagedSecret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.key_var)
    }
}

impl schemars::JsonSchema for ManagedSecret {
    fn schema_name() -> String {
        "ManagedSecret".to_string()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        // ManagedSecret is serialized as a string (the key_var name)
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

/// What an app-secret write did: stored a key the app did not have, or replaced
/// the value of one it did. The write is an upsert, and only the caller can say
/// which happened — so it reports it rather than leaving the client to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretWrite {
    Created,
    Updated,
}

#[derive(Debug, Clone)]
pub struct SecretManagerService {
    encryption_key: [u8; 32],
    cache: Arc<RwLock<HashMap<String, CachedSecret>>>,
    project_id: Uuid,
}

#[derive(Debug, Clone)]
struct CachedSecret {
    value: String,
    cached_at: chrono::DateTime<chrono::Utc>,
    ttl_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct CreateSecretParams {
    pub name: String,
    pub value: String,
    pub description: Option<String>,
    pub created_by: Uuid,
}

#[derive(Debug, Clone)]
pub struct UpdateSecretParams {
    pub value: Option<String>,
    pub description: Option<String>,
    pub updated_by: Uuid,
}

#[derive(Debug, Clone)]
pub struct SecretInfo {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Uuid,
    pub updated_by: Option<Uuid>,
    pub is_active: bool,
}

impl SecretManagerService {
    pub fn new(project_id: Uuid) -> Self {
        let encryption_key = get_encryption_key();
        Self {
            encryption_key,
            cache: Arc::new(RwLock::new(HashMap::new())),
            project_id,
        }
    }

    fn encrypt_value(&self, value: &str) -> Result<String, OxyError> {
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.encryption_key));
        let nonce = AeadNonce::<Aes256Gcm>::generate();

        let ciphertext = cipher
            .encrypt(&nonce, value.as_bytes())
            .map_err(|e| OxyError::SecretManager(format!("Encryption failed: {e}")))?;

        // Combine nonce and ciphertext, then base64 encode
        let mut combined = nonce.to_vec();
        combined.extend_from_slice(&ciphertext);

        Ok(general_purpose::STANDARD.encode(&combined))
    }

    // Same framing and same coverage gap as `crate::utils::decrypt_value` —
    // see the TODO(secrets) there before changing the layout below.
    fn decrypt_value(&self, encrypted_value: &str) -> Result<String, OxyError> {
        let combined = general_purpose::STANDARD
            .decode(encrypted_value)
            .map_err(|e| OxyError::SecretManager(format!("Invalid encrypted value format: {e}")))?;

        if combined.len() < 12 {
            return Err(OxyError::SecretManager(
                "Invalid encrypted value: too short".to_string(),
            ));
        }

        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = <&Nonce<_>>::try_from(nonce_bytes).map_err(|_| {
            OxyError::SecretManager("Invalid encrypted value: bad nonce".to_string())
        })?;

        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.encryption_key));
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| OxyError::SecretManager(format!("Decryption failed: {e}")))?;

        String::from_utf8(plaintext)
            .map_err(|e| OxyError::SecretManager(format!("Invalid UTF-8 in decrypted value: {e}")))
    }

    /// A caller-chosen secret name: 1–255 of alphanumerics, `_`, `-` and `.`
    /// — never a `/`, so it is one segment of a system-built path such as
    /// `apps/<app_id>/<KEY>`.
    pub fn validate_secret_name(name: &str) -> Result<(), OxyError> {
        if name.is_empty() {
            return Err(OxyError::SecretManager(
                "Secret name cannot be empty".to_string(),
            ));
        }

        if name.len() > 255 {
            return Err(OxyError::SecretManager(
                "Secret name cannot be longer than 255 characters".to_string(),
            ));
        }

        // Check for valid characters (alphanumeric, underscore, hyphen, dot)
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(OxyError::SecretManager(
                "Secret name can only contain alphanumeric characters, underscores, hyphens, and dots".to_string(),
            ));
        }

        Ok(())
    }

    fn sanitize_secret_value(value: &str) -> Result<String, OxyError> {
        if value.is_empty() {
            return Err(OxyError::SecretManager(
                "Secret value cannot be empty".to_string(),
            ));
        }

        if value.len() > 10000 {
            return Err(OxyError::SecretManager(
                "Secret value cannot be longer than 10000 characters".to_string(),
            ));
        }

        // Trim whitespace
        Ok(value.trim().to_string())
    }

    pub async fn create_secret<C>(
        &self,
        db: &C,
        params: CreateSecretParams,
    ) -> Result<SecretInfo, OxyError>
    where
        C: sea_orm::ConnectionTrait,
    {
        tracing::info!("Creating secret: {}", params.name);
        Self::validate_secret_name(&params.name)?;
        let sanitized_value = Self::sanitize_secret_value(&params.value)?;

        // Check if secret with this name already exists
        let existing = Secret::find()
            .filter(secrets::Column::Name.eq(&params.name))
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .filter(secrets::Column::IsActive.eq(true))
            .one(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        tracing::debug!(
            "Checking for existing secret with name '{}': {:?}",
            params.name,
            existing
        );
        if existing.is_some() {
            tracing::warn!(
                "Attempted to create secret with duplicate name: {}",
                params.name
            );
            return Err(OxyError::SecretManager(format!(
                "Secret with name '{}' already exists",
                params.name
            )));
        }

        let encrypted_value = self.encrypt_value(&sanitized_value)?;
        let now = chrono::Utc::now();

        let secret_model = SecretActiveModel {
            id: Set(Uuid::new_v4()),
            name: Set(params.name.clone()),
            encrypted_value: Set(encrypted_value),
            description: Set(params.description),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
            created_by: Set(params.created_by),
            updated_by: sea_orm::ActiveValue::NotSet,
            is_active: Set(true),
            project_id: Set(self.project_id),
        };

        tracing::info!("Inserting new secret: {}", secret_model.name.as_ref());

        let saved_secret = secret_model.insert(db).await.map_err(|e| {
            tracing::error!("Failed to insert secret: {}", e);
            OxyError::Database(e.to_string())
        })?;

        tracing::info!("Secret created successfully: {}", saved_secret.name);
        self.invalidate_cache(&params.name).await;

        Ok(SecretInfo {
            id: saved_secret.id,
            name: saved_secret.name,
            description: saved_secret.description,
            created_at: saved_secret.created_at.into(),
            updated_at: saved_secret.updated_at.into(),
            created_by: saved_secret.created_by,
            updated_by: saved_secret.updated_by,
            is_active: saved_secret.is_active,
        })
    }

    /// Get secret metadata by UUID (without decrypting the value)
    pub async fn get_secret_by_id(&self, id: Uuid) -> Option<SecretInfo> {
        let db = establish_connection().await.ok()?;
        let secret = Secret::find()
            .filter(secrets::Column::Id.eq(id))
            .filter(secrets::Column::IsActive.eq(true))
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .one(&db)
            .await
            .ok()
            .flatten()?;
        Some(SecretInfo {
            id: secret.id,
            name: secret.name,
            description: secret.description,
            created_at: secret.created_at.into(),
            updated_at: secret.updated_at.into(),
            created_by: secret.created_by,
            updated_by: secret.updated_by,
            is_active: secret.is_active,
        })
    }

    /// Get a decrypted secret value by UUID — single DB roundtrip.
    /// `Ok(None)` means the row is genuinely absent. `Err` means the row is
    /// there and this process could not read it.
    ///
    /// The distinction is the point. Collapsing both into `None` made the
    /// caller answer 404 — "this secret does not exist" — when the truth was
    /// "this node cannot decrypt it", which is what happens on a fleet where
    /// the encryption key lives in each node's own OXY_STATE_DIR. A secret
    /// written through one replica then reads as missing from another, and a
    /// missing secret is something an operator goes and re-creates rather than
    /// investigates. The failure has to be able to say it is a failure.
    pub async fn get_secret_value_by_id(&self, id: Uuid) -> Result<Option<String>, OxyError> {
        let db = establish_connection()
            .await
            .map_err(|e| OxyError::SecretManager(format!("database connection failed: {e}")))?;
        let secret = Secret::find()
            .filter(secrets::Column::Id.eq(id))
            .filter(secrets::Column::IsActive.eq(true))
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .one(&db)
            .await
            .map_err(|e| OxyError::SecretManager(format!("secret lookup failed: {e}")))?;
        let Some(secret) = secret else {
            return Ok(None);
        };
        match self.decrypt_value(&secret.encrypted_value) {
            Ok(value) => {
                self.cache_value(&secret.name, &value).await;
                Ok(Some(value))
            }
            Err(e) => {
                tracing::error!("Failed to decrypt secret {}: {}", id, e);
                Err(OxyError::SecretManager(format!(
                    "secret {id} exists but could not be decrypted by this process: {e}"
                )))
            }
        }
    }

    pub async fn get_secret(&self, name: &str) -> Option<String> {
        // Check cache first
        if let Some(cached_value) = self.get_from_cache(name).await {
            return Some(cached_value);
        }

        let db = establish_connection().await;

        let secret = match db {
            Ok(conn) => {
                let rs = Secret::find()
                    .filter(secrets::Column::Name.eq(name))
                    .filter(secrets::Column::IsActive.eq(true))
                    .filter(secrets::Column::ProjectId.eq(self.project_id))
                    .one(&conn)
                    .await;
                match rs {
                    Ok(secret) => secret,
                    Err(e) => {
                        tracing::error!("Failed to query secret: {}", e);
                        return None;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to establish database connection: {}", e);
                return None;
            }
        };

        if let Some(secret) = secret {
            let decrypted_value = self.decrypt_value(&secret.encrypted_value);
            match decrypted_value {
                Ok(value) => {
                    self.cache_value(name, &value).await;

                    Some(value)
                }
                Err(e) => {
                    tracing::error!("Failed to decrypt secret value: {}", e);
                    None
                }
            }
        } else {
            None
        }
    }

    /// List all secrets (without values)
    pub async fn list_secrets<C>(&self, db: &C) -> Result<Vec<SecretInfo>, OxyError>
    where
        C: sea_orm::ConnectionTrait,
    {
        let secrets = Secret::find()
            .filter(secrets::Column::IsActive.eq(true))
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .order_by_asc(secrets::Column::Name)
            .all(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        Ok(secrets
            .into_iter()
            .map(|secret| SecretInfo {
                id: secret.id,
                name: secret.name,
                description: secret.description,
                created_at: secret.created_at.into(),
                updated_at: secret.updated_at.into(),
                created_by: secret.created_by,
                updated_by: secret.updated_by,
                is_active: secret.is_active,
            })
            .collect())
    }

    pub async fn update_secret<C>(
        &self,
        db: &C,
        name: &str,
        params: UpdateSecretParams,
    ) -> Result<SecretInfo, OxyError>
    where
        C: sea_orm::ConnectionTrait,
    {
        let secret = Secret::find()
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .filter(secrets::Column::Name.eq(name))
            .filter(secrets::Column::IsActive.eq(true))
            .one(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        let secret = secret.ok_or_else(|| {
            OxyError::SecretManager(format!("Secret with name '{name}' not found"))
        })?;

        let mut secret_model: SecretActiveModel = secret.into();

        if let Some(new_value) = params.value {
            let sanitized_value = Self::sanitize_secret_value(&new_value)?;
            let encrypted_value = self.encrypt_value(&sanitized_value)?;
            secret_model.encrypted_value = Set(encrypted_value);
        }

        if let Some(new_description) = params.description {
            secret_model.description = Set(Some(new_description));
        }

        secret_model.updated_at = Set(chrono::Utc::now().into());
        secret_model.updated_by = Set(Some(params.updated_by));

        let updated_secret = secret_model
            .update(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        self.invalidate_cache(name).await;

        Ok(SecretInfo {
            id: updated_secret.id,
            name: updated_secret.name,
            description: updated_secret.description,
            created_at: updated_secret.created_at.into(),
            updated_at: updated_secret.updated_at.into(),
            created_by: updated_secret.created_by,
            updated_by: updated_secret.updated_by,
            is_active: updated_secret.is_active,
        })
    }

    /// Upsert an **app-scoped** secret: `apps/<app_id>/<key>`.
    ///
    /// This is the write counterpart to the `resolve_function_env` reader in the
    /// custom-app functions runtime — it lets an Oxy Function (e.g. a scheduled
    /// token-refresher) persist state into the same `apps/<app_id>/` namespace
    /// `ctx.env` reads. Only the caller-supplied `key` is validated against the
    /// name charset; the `apps/<app_id>/` prefix is system-generated, so the full
    /// name legitimately contains `/` (which `validate_secret_name` forbids) —
    /// hence this dedicated path rather than `create_secret`/`update_secret`.
    ///
    /// `actor` is stamped as `created_by`/`updated_by` (the `secrets.created_by`
    /// FK is non-null with `on_delete = Restrict`, so it must be a real user —
    /// the invoking user for a route call, or the app owner for a scheduled call).
    pub async fn set_app_secret(
        &self,
        db: &sea_orm::DatabaseConnection,
        app_id: Uuid,
        key: &str,
        value: &str,
        actor: Uuid,
    ) -> Result<SecretWrite, OxyError> {
        self.set_app_secret_in(db, app_id, None, key, value, actor)
            .await
    }

    /// The storage name of an app secret: `apps/<app_id>/<key>` for
    /// production (`environment: None`), `apps/<app_id>/<env>/<key>` for a
    /// non-production environment of the app. One definition, shared by this
    /// writer and the `ctx.env` reader, so the two cannot drift.
    pub fn app_secret_name(app_id: Uuid, environment: Option<&str>, key: &str) -> String {
        match environment {
            None => format!("apps/{app_id}/{key}"),
            Some(env) => format!("apps/{app_id}/{env}/{key}"),
        }
    }

    /// [`Self::set_app_secret`] into one environment's path of the app
    /// (`apps/<app_id>/<env>/<key>`), or production's for `None`. The
    /// environment name is validated like a key, so it is one path segment
    /// and can never reach another app's or production's path.
    pub async fn set_app_secret_in(
        &self,
        db: &sea_orm::DatabaseConnection,
        app_id: Uuid,
        environment: Option<&str>,
        key: &str,
        value: &str,
        actor: Uuid,
    ) -> Result<SecretWrite, OxyError> {
        // Validate only the caller's key (and environment); the prefix is
        // trusted/system-built.
        Self::validate_secret_name(key)?;
        if let Some(env) = environment {
            Self::validate_secret_name(env)?;
        }
        let sanitized = Self::sanitize_secret_value(value)?;
        let name = Self::app_secret_name(app_id, environment, key);
        let encrypted = self.encrypt_value(&sanitized)?;
        let now = chrono::Utc::now();

        // Serialize concurrent writers for the SAME (project, name). The table
        // has no unique constraint on active names, so a bare find-then-write
        // would let two concurrent `ctx.secrets.set` both see `None` and both
        // INSERT — two active rows, nondeterministic reads. A transaction-scoped
        // advisory lock keyed on (project, name) makes the second writer queue
        // behind the first (then take the UPDATE path); it releases on
        // commit/rollback, and different secrets never contend.
        let txn = db
            .begin()
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;
        txn.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock($1)",
            [Self::advisory_lock_key(self.project_id, &name).into()],
        ))
        .await
        .map_err(|e| OxyError::Database(e.to_string()))?;

        let existing = Secret::find()
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .filter(secrets::Column::Name.eq(&name))
            .filter(secrets::Column::IsActive.eq(true))
            .one(&txn)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        let write = match existing {
            Some(row) => {
                let mut model: SecretActiveModel = row.into();
                model.encrypted_value = Set(encrypted);
                model.updated_at = Set(now.into());
                model.updated_by = Set(Some(actor));
                model
                    .update(&txn)
                    .await
                    .map_err(|e| OxyError::Database(e.to_string()))?;
                SecretWrite::Updated
            }
            None => {
                let model = SecretActiveModel {
                    id: Set(Uuid::new_v4()),
                    name: Set(name.clone()),
                    encrypted_value: Set(encrypted),
                    description: Set(None),
                    created_at: Set(now.into()),
                    updated_at: Set(now.into()),
                    created_by: Set(actor),
                    updated_by: sea_orm::ActiveValue::NotSet,
                    is_active: Set(true),
                    project_id: Set(self.project_id),
                };
                model
                    .insert(&txn)
                    .await
                    .map_err(|e| OxyError::Database(e.to_string()))?;
                SecretWrite::Created
            }
        };

        txn.commit()
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;
        self.invalidate_cache(&name).await;
        Ok(write)
    }

    /// Stable 64-bit key for `pg_advisory_xact_lock`, derived from the
    /// (project, secret-name) pair so concurrent writers to the SAME secret
    /// serialize while different secrets stay lock-free. Deterministic across
    /// processes (fixed-key SipHash), so it also serializes across replicas; a
    /// rare hash collision only makes two unrelated names share a lock (safe).
    fn advisory_lock_key(project_id: Uuid, name: &str) -> i64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        project_id.hash(&mut h);
        name.hash(&mut h);
        h.finish() as i64
    }

    /// Delete a secret (soft delete)
    pub async fn delete_secret<C>(&self, db: &C, name: &str) -> Result<(), OxyError>
    where
        C: sea_orm::ConnectionTrait,
    {
        let secret = Secret::find()
            .filter(secrets::Column::ProjectId.eq(self.project_id))
            .filter(secrets::Column::Name.eq(name))
            .filter(secrets::Column::IsActive.eq(true))
            .one(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        let secret = secret.ok_or_else(|| {
            OxyError::SecretManager(format!("Secret with name '{name}' not found"))
        })?;

        let mut secret_model: SecretActiveModel = secret.into();
        secret_model.is_active = Set(false);
        secret_model.updated_at = Set(chrono::Utc::now().into());

        secret_model
            .update(db)
            .await
            .map_err(|e| OxyError::Database(e.to_string()))?;

        // Remove from cache
        self.invalidate_cache(name).await;

        Ok(())
    }

    // Cache management methods
    async fn get_from_cache(&self, name: &str) -> Option<String> {
        let cache = self.cache.read().await;
        if let Some(cached) = cache.get(name) {
            let now = chrono::Utc::now();
            let age = now.timestamp() as u64 - cached.cached_at.timestamp() as u64;

            if age < cached.ttl_seconds {
                return Some(cached.value.clone());
            }
        }
        None
    }

    async fn cache_value(&self, name: &str, value: &str) {
        let mut cache = self.cache.write().await;
        cache.insert(
            name.to_string(),
            CachedSecret {
                value: value.to_string(),
                cached_at: chrono::Utc::now(),
                ttl_seconds: 300, // 5 minutes
            },
        );
    }

    async fn invalidate_cache(&self, name: &str) {
        let mut cache = self.cache.write().await;
        cache.remove(name);
    }

    /// Clear all cached secrets
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }
}
