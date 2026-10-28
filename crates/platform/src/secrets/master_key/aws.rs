//! KMS through the AWS SDK.

use aws_credential_types::provider::{ProvideCredentials, error::CredentialsError};
use aws_sdk_kms::primitives::Blob;
use tokio::sync::OnceCell;

use super::{KeyLookup, Kms, MASTER_KEY_ROW};

/// Bound into every KMS call on this row: a ciphertext made for something else
/// under the same KMS key does not decrypt as the master key.
const CONTEXT_KEY: &str = "oxy:server_keys";

/// KMS through the AWS SDK, with the process's default credentials (Pod
/// Identity in the clusters). The AWS configuration is loaded on the first
/// call, so a boot that never calls KMS never resolves a region or
/// credentials.
#[derive(Default)]
pub struct AwsKms {
    conf: OnceCell<aws_config::SdkConfig>,
}

/// The region a KMS call for `key_id` goes to. A key ARN names its region,
/// and the call must go there whatever the environment says: a ciphertext
/// opens only in the region of the key that sealed it. An alias goes to the
/// region the environment names (`resolved`: `AWS_REGION`,
/// `AWS_DEFAULT_REGION`, the profile, IMDS), else `OXY_DATABASE_REGION`, which
/// a deployment on RDS IAM auth already sets to its own region.
fn region_for(key_id: &str, resolved: Option<String>, database: Option<String>) -> Option<String> {
    key_id
        .strip_prefix("arn:aws:kms:")
        .and_then(|r| r.split(':').next())
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .or(resolved)
        .or(database.filter(|r| !r.trim().is_empty()))
}

impl AwsKms {
    pub fn new() -> Self {
        Self::default()
    }

    async fn conf(&self) -> &aws_config::SdkConfig {
        self.conf
            .get_or_init(|| aws_config::defaults(aws_config::BehaviorVersion::latest()).load())
            .await
    }

    /// A client for the region [`region_for`] picks, or `None` without one.
    async fn client(&self, key_id: &str) -> Option<aws_sdk_kms::Client> {
        let conf = self.conf().await;
        let region = region_for(
            key_id,
            conf.region().map(|r| r.to_string()),
            std::env::var("OXY_DATABASE_REGION").ok(),
        )?;
        let kms_conf = aws_sdk_kms::config::Builder::from(conf)
            .region(aws_config::Region::new(region))
            .build();
        Some(aws_sdk_kms::Client::from_conf(kms_conf))
    }

    async fn client_or_err(&self, key_id: &str) -> Result<aws_sdk_kms::Client, String> {
        self.client(key_id).await.ok_or_else(|| {
            format!("no AWS region for {key_id}: AWS_REGION and OXY_DATABASE_REGION are not set")
        })
    }

    /// `Some(why)` when this process has no AWS credentials at all, as outside
    /// AWS. A provider that exists and fails (Pod Identity's agent down, say)
    /// is an error, not this.
    async fn missing_credentials(&self) -> Result<Option<String>, String> {
        let Some(provider) = self.conf().await.credentials_provider() else {
            return Ok(Some("no AWS credentials provider".to_string()));
        };
        match provider.provide_credentials().await {
            Ok(_) => Ok(None),
            Err(e @ CredentialsError::CredentialsNotLoaded(_)) => Ok(Some(format!(
                "no AWS credentials: {}",
                aws_sdk_kms::error::DisplayErrorContext(e)
            ))),
            Err(e) => Err(sdk_error(e)),
        }
    }
}

fn sdk_error<E: std::error::Error + 'static>(e: E) -> String {
    aws_sdk_kms::error::DisplayErrorContext(e).to_string()
}

impl Kms for AwsKms {
    async fn find_key(&self, key_id: &str) -> Result<KeyLookup, String> {
        let Some(client) = self.client(key_id).await else {
            return Ok(KeyLookup::NoAws(
                "no AWS region: AWS_REGION and OXY_DATABASE_REGION are not set".to_string(),
            ));
        };
        if let Some(why) = self.missing_credentials().await? {
            return Ok(KeyLookup::NoAws(why));
        }
        match client.describe_key().key_id(key_id).send().await {
            Ok(_) => Ok(KeyLookup::Found),
            Err(e)
                if e.as_service_error()
                    .is_some_and(|s| s.is_not_found_exception()) =>
            {
                Ok(KeyLookup::NotFound)
            }
            Err(e) => Err(sdk_error(e)),
        }
    }

    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<(String, Vec<u8>), String> {
        let out = self
            .client_or_err(key_id)
            .await?
            .encrypt()
            .key_id(key_id)
            .plaintext(Blob::new(plaintext))
            .encryption_context(CONTEXT_KEY, MASTER_KEY_ROW)
            .send()
            .await
            .map_err(sdk_error)?;
        let ciphertext = out
            .ciphertext_blob()
            .ok_or("KMS returned no ciphertext")?
            .as_ref()
            .to_vec();
        let sealed_by = out.key_id().ok_or("KMS did not say which key sealed it")?;
        Ok((sealed_by.to_string(), ciphertext))
    }

    async fn decrypt(
        &self,
        sealed_by: &str,
        ciphertext: &[u8],
    ) -> Result<(String, Vec<u8>), String> {
        // No key id: KMS takes the key from the ciphertext.
        let out = self
            .client_or_err(sealed_by)
            .await?
            .decrypt()
            .ciphertext_blob(Blob::new(ciphertext))
            .encryption_context(CONTEXT_KEY, MASTER_KEY_ROW)
            .send()
            .await
            .map_err(sdk_error)?;
        let opened_by = out.key_id().ok_or("KMS did not say which key opened it")?;
        let plaintext = out
            .plaintext()
            .ok_or("KMS returned no plaintext")?
            .as_ref()
            .to_vec();
        Ok((opened_by.to_string(), plaintext))
    }
}

#[cfg(test)]
mod tests {
    use super::region_for;

    #[test]
    fn a_key_arn_decides_the_region() {
        let arn = "arn:aws:kms:eu-west-1:1:key/k";
        assert_eq!(
            region_for(arn, Some("us-west-2".into()), None).as_deref(),
            Some("eu-west-1")
        );
    }

    #[test]
    fn an_alias_uses_the_environment_then_the_database_region() {
        let alias = "alias/oxy-dev-oxy-master-key";
        assert_eq!(
            region_for(alias, Some("us-west-2".into()), Some("eu-west-1".into())).as_deref(),
            Some("us-west-2")
        );
        assert_eq!(
            region_for(alias, None, Some("us-west-2".into())).as_deref(),
            Some("us-west-2")
        );
        assert_eq!(region_for(alias, None, None), None);
        assert_eq!(region_for(alias, None, Some(" ".into())), None);
    }
}
