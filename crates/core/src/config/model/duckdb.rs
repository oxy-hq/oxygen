use crate::adapters::secrets::SecretsManager;
use crate::config::validate::ValidationContext;
use crate::connector::{
    ConnectionStringFormatter, ConnectionStringParser, PostgresConnectionString,
};
use crate::service::secret_manager::ManagedSecret;
use garde::Validate;
use oxy_shared::errors::OxyError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
#[serde(tag = "s3_secret_type", rename_all = "snake_case")]
pub enum S3StorageSecret {
    Config {
        #[garde(length(min = 1))]
        key_id: String,
        /// The secret key, referenced by a key variable name stored in SecretManager.
        /// In YAML, specify the secret key name: `secret: AWS_S3_SECRET`
        #[serde(default = "default_s3_secret_var")]
        #[garde(skip)]
        secret: ManagedSecret,
        #[serde(default = "default_s3_region")]
        #[garde(length(min = 1))]
        region: String,
        #[garde(length(min = 1))]
        endpoint_url: Option<String>,
        #[serde(default = "default_use_ssl")]
        #[garde(skip)]
        use_ssl: bool,
        #[serde(default = "default_url_style")]
        #[garde(length(min = 1))]
        url_style: String,
    },
    CredentialChain {
        #[serde(default = "default_s3_chain")]
        #[garde(length(min = 1))]
        chain: Option<String>,
        #[serde(default = "default_s3_region")]
        #[garde(length(min = 1))]
        region: String,
    },
}

impl S3StorageSecret {
    /// Generate the DuckDB secret statement, and the secret value it carries.
    ///
    /// For `Config` variant, the secret value is retrieved from the `SecretsManager`.
    pub async fn to_duckdb_secret_stmt(
        &self,
        secrets_manager: &SecretsManager,
    ) -> Result<(String, Option<String>), OxyError> {
        match self {
            S3StorageSecret::Config {
                key_id,
                secret,
                region,
                endpoint_url,
                url_style,
                use_ssl,
            } => {
                let secret_value = secret.expose_str_with_adapter(secrets_manager).await?;

                let stmt = format!(
                    "
CREATE OR REPLACE SECRET s3_secret (
    TYPE s3,
    PROVIDER config,
    URL_STYLE '{}',
    ENDPOINT '{}',
    USE_SSL {},
    KEY_ID '{}',
    SECRET '{}',
    REGION '{}'
)",
                    escape_sql_string(url_style),
                    escape_sql_string(
                        &endpoint_url
                            .clone()
                            .unwrap_or(format!("s3.{}.amazonaws.com", region))
                    ),
                    use_ssl,
                    escape_sql_string(key_id),
                    escape_sql_string(&secret_value),
                    escape_sql_string(region)
                );
                Ok((stmt, Some(secret_value)))
            }
            S3StorageSecret::CredentialChain { chain, region } => {
                let chain_stmt = if let Some(chain_str) = chain {
                    format!("CHAIN '{}'", escape_sql_string(chain_str))
                } else {
                    "".to_string()
                };
                let stmt = format!(
                    "
CREATE OR REPLACE SECRET s3_secret (
    TYPE s3,
    PROVIDER credential_chain,
    {},
    REGION '{}'
)",
                    chain_stmt,
                    escape_sql_string(region)
                );
                Ok((stmt, None))
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct CatalogConfig {
    /// The catalog path, referenced by a key variable name stored in SecretManager.
    /// The secret value should be a postgres connection string URL like:
    /// `postgres://user:pass@localhost:5432/catalog_db`
    #[serde(default = "default_catalog_path")]
    #[garde(skip)]
    catalog_path: ManagedSecret,
    #[garde(length(min = 1))]
    schema_name: String,
}

impl CatalogConfig {
    /// Get the catalog path formatted for DuckDB, and the catalog password.
    ///
    /// Retrieves the postgres connection string from the secrets manager,
    /// parses it, and formats it for DuckDB's postgres extension. The path
    /// carries no password; see [`DuckLakeConfig::to_duckdb_attach_stmt`].
    async fn to_duckdb_attach_path(
        &self,
        secrets_manager: &SecretsManager,
    ) -> Result<(String, Option<String>), OxyError> {
        let connection_string = self
            .catalog_path
            .expose_str_with_adapter(secrets_manager)
            .await?;

        // Parse the postgres URL and format it for DuckDB. The parse error
        // never repeats the URL, which holds the password.
        let parsed = PostgresConnectionString::parse(&connection_string).map_err(|e| {
            OxyError::ConfigurationError(format!("Invalid catalog_path connection string: {}", e))
        })?;

        Ok((parsed.to_duckdb_format(), parsed.password))
    }

    fn to_duckdb_attach_params(&self) -> Vec<String> {
        vec![format!(
            "METADATA_SCHEMA '{}'",
            escape_sql_string(&self.schema_name)
        )]
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct StorageConfig {
    #[garde(length(min = 1))]
    pub data_path: String,
    #[serde(flatten)]
    #[garde(dive)]
    pub storage_secret: S3StorageSecret,
}

impl StorageConfig {
    fn to_duckdb_attach_params(&self) -> Vec<String> {
        vec![format!(
            "DATA_PATH '{}'",
            escape_sql_string(&self.data_path)
        )]
    }
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
pub struct DuckLakeConfig {
    #[garde(dive)]
    #[serde(flatten)]
    pub catalog: CatalogConfig,
    #[garde(dive)]
    #[serde(flatten)]
    pub storage: StorageConfig,
}

impl DuckLakeConfig {
    /// Generate the DuckDB attach statements.
    ///
    /// The secret values are retrieved from the `SecretsManager`. The catalog
    /// password goes in an unnamed `TYPE postgres` secret, never in the
    /// `ATTACH` path: DuckDB and DuckLake repeat that path in their errors,
    /// and the postgres extension adds the unnamed secret's options to any
    /// attach that names no secret of its own.
    pub async fn to_duckdb_attach_stmt(
        &self,
        secrets_manager: &SecretsManager,
    ) -> Result<DuckLakeAttach, OxyError> {
        // Retrieve and format the catalog path (postgres connection string)
        let (catalog_path, catalog_password) =
            self.catalog.to_duckdb_attach_path(secrets_manager).await?;
        let catalog_param = self.catalog.to_duckdb_attach_params();
        let storage_param = self.storage.to_duckdb_attach_params();
        let params = catalog_param
            .iter()
            .chain(storage_param.iter())
            .cloned()
            .collect::<Vec<String>>();
        let (storage_secret_stmt, storage_secret_value) = self
            .storage
            .storage_secret
            .to_duckdb_secret_stmt(secrets_manager)
            .await?;

        let mut statements = vec![storage_secret_stmt];
        if let Some(password) = &catalog_password {
            statements.push(format!(
                "CREATE OR REPLACE SECRET (TYPE postgres, PASSWORD '{}')",
                escape_sql_string(password)
            ));
        }
        statements.push(format!(
            "ATTACH '{}' AS ducklake ({})",
            escape_sql_string(&format!("ducklake:{catalog_path}")),
            params.join(", ")
        ));
        statements.push("USE ducklake".to_string());

        Ok(DuckLakeAttach {
            statements,
            secret_values: storage_secret_value
                .into_iter()
                .chain(catalog_password)
                .collect(),
        })
    }
}

/// The statements that attach a DuckLake, and the secret values inside them.
///
/// The statements carry credentials (the S3 secret key and the catalog
/// password, each in a `CREATE SECRET`), and a DuckDB error about one can
/// quote it: a parser error repeats the statement. So neither a statement
/// nor an error from running one is logged or returned as it is; it goes
/// through [`Self::redact`] first. `Debug` prints only the count.
#[derive(Clone)]
pub struct DuckLakeAttach {
    pub statements: Vec<String>,
    pub secret_values: Vec<String>,
}

impl DuckLakeAttach {
    /// `text` with every secret value replaced by `***`.
    pub fn redact(&self, text: &str) -> String {
        redact_secret_values(text, &self.secret_values)
    }
}

impl std::fmt::Debug for DuckLakeAttach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckLakeAttach")
            .field("statements", &self.statements.len())
            .finish_non_exhaustive()
    }
}

/// `text` with each non-empty value in `secrets` replaced by `***`, in each
/// spelling it can take in DuckDB text: as written, inside a SQL literal
/// (`'` doubled), and inside a quoted libpq value (`\` and `'` escaped).
pub fn redact_secret_values(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        let libpq = secret.replace('\\', "\\\\").replace('\'', "\\'");
        // Longest spelling first, so a shorter one cannot leave a tail behind.
        let mut spellings = [escape_sql_string(secret), libpq, secret.clone()];
        spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for spelling in spellings {
            out = out.replace(&spelling, "***");
        }
    }
    out
}

#[derive(Serialize, Deserialize, Debug, Validate, Clone, JsonSchema)]
#[garde(context(ValidationContext))]
#[serde(untagged)]
pub enum DuckDBOptions {
    Local {
        #[serde(alias = "dataset", rename = "dataset")]
        #[garde(length(min = 1))]
        file_search_path: String,
    },
    File {
        #[garde(length(min = 1))]
        path: String,
    },
    DuckLake(#[garde(dive)] DuckLakeConfig),
}

/// Compiler-produced S3 mirror of a **local-file** DuckDB warehouse
/// (`Local` / `File` modes). Present **only in the compiled config**, never in
/// `config.yml`: when the compile worker uploads a workspace's on-disk DuckDB
/// data to S3, it records the location here so a stateless serve replica —
/// which has no working copy — can read the data over `httpfs` instead of the
/// absent local path. Its mere presence means "this is a fleet read"; local /
/// IDE reads come from the filesystem and never carry it.
///
/// Credentials are the pod's instance role (S3 `credential_chain`), so nothing
/// sensitive is stored here.
#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq)]
pub struct DuckDbS3Mirror {
    /// Bucket the data was mirrored into (`OXY_COMPILE_BLOB_S3_BUCKET`).
    pub bucket: String,
    /// AWS region for the S3 secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Custom endpoint (MinIO / LocalStack); when set, path-style addressing is
    /// used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    /// `Local` mode: one mirrored data file per table (table name = slugified
    /// file stem, matching local registration).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<DuckDbS3Table>,
    /// `File` mode: the mirrored `.duckdb` object key (attached read-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_key: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema, PartialEq)]
pub struct DuckDbS3Table {
    pub table: String,
    pub key: String,
    /// `parquet` | `csv`.
    pub format: String,
}

fn default_use_ssl() -> bool {
    false
}

fn default_s3_region() -> String {
    "us-east-1".to_string()
}

fn default_s3_secret_var() -> ManagedSecret {
    ManagedSecret::new("DUCKLAKE_S3_SECRET".to_string())
}

fn default_s3_chain() -> Option<String> {
    Some("sso;config".to_string())
}

fn default_catalog_path() -> ManagedSecret {
    ManagedSecret::new("DUCKLAKE_CATALOG_PATH".to_string())
}

fn default_url_style() -> String {
    "path".to_string()
}

fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "p/ss w0rd'x";
    const S3_SECRET: &str = "s3-secret-value";

    fn ducklake_config() -> DuckLakeConfig {
        serde_yaml::from_str(
            "catalog_path: DLT_TEST_CATALOG\n\
             schema_name: ducklake\n\
             data_path: s3://bucket/\n\
             s3_secret_type: config\n\
             key_id: AKIA0000\n\
             secret: DLT_TEST_S3_SECRET\n",
        )
        .unwrap()
    }

    /// The catalog password never sits in the `ATTACH` path, which DuckDB and
    /// DuckLake repeat in their errors. It is percent-decoded from the URL,
    /// and every statement or error that could quote it can be scrubbed.
    #[tokio::test]
    async fn the_attach_path_carries_no_password() {
        // SAFETY: nextest runs each test in its own process.
        unsafe {
            std::env::set_var(
                "DLT_TEST_CATALOG",
                "postgres://ducklake:p%2Fss%20w0rd'x@127.0.0.1:5432/ducklake_catalog?sslmode=require",
            );
            std::env::set_var("DLT_TEST_S3_SECRET", S3_SECRET);
        }
        let sm = SecretsManager::from_environment().unwrap();

        let attach = ducklake_config().to_duckdb_attach_stmt(&sm).await.unwrap();

        let attach_stmt = attach
            .statements
            .iter()
            .find(|s| s.starts_with("ATTACH"))
            .unwrap();
        assert_eq!(
            attach_stmt,
            "ATTACH 'ducklake:postgres:user=ducklake host=127.0.0.1 port=5432 \
             dbname=ducklake_catalog sslmode=require' AS ducklake \
             (METADATA_SCHEMA 'ducklake', DATA_PATH 's3://bucket/')"
        );
        assert!(
            attach.statements.contains(
                &"CREATE OR REPLACE SECRET (TYPE postgres, PASSWORD 'p/ss w0rd''x')".to_string()
            ),
            "{attach:?}"
        );
        assert_eq!(attach.secret_values, vec![S3_SECRET, PASSWORD]);

        for stmt in &attach.statements {
            let redacted = attach.redact(stmt);
            assert!(!redacted.contains("p/ss w0rd"), "{redacted}");
            assert!(!redacted.contains(S3_SECRET), "{redacted}");
        }
        let debug = format!("{attach:?}");
        assert!(
            !debug.contains("w0rd") && !debug.contains(S3_SECRET),
            "{debug}"
        );
    }

    #[test]
    fn redaction_covers_each_spelling_of_a_secret() {
        let secrets = vec![r"a'b\c".to_string(), String::new()];
        let text = r"raw a'b\c sql 'a''b\c' libpq 'a\'b\\c' empty stays";
        assert_eq!(
            redact_secret_values(text, &secrets),
            "raw *** sql '***' libpq '***' empty stays"
        );
    }
}
