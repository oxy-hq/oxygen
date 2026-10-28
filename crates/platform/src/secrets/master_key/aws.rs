//! KMS through the AWS SDK.

use aws_sdk_kms::primitives::Blob;

use super::{Kms, MASTER_KEY_ROW};

/// Bound into every KMS call on this row: a ciphertext made for something else
/// under the same KMS key does not decrypt as the master key.
const CONTEXT_KEY: &str = "oxy:server_keys";

/// KMS through the AWS SDK, with the process's default credentials (Pod
/// Identity in the clusters).
pub struct AwsKms {
    client: aws_sdk_kms::Client,
}

/// The region a KMS call for `key_id` goes to. A key ARN names its region,
/// and the call must go there whatever the environment says. An alias or bare
/// id goes to the region the environment names (`resolved`: `AWS_REGION`,
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
    /// `Err` when no region can be found for `key_id`: a call without one fails
    /// with an error that does not say which setting is missing.
    pub async fn from_env(key_id: &str) -> Result<Self, String> {
        let resolved = aws_config::default_provider::region::DefaultRegionChain::builder()
            .build()
            .region()
            .await
            .map(|r| r.to_string());
        let region = region_for(key_id, resolved, std::env::var("OXY_DATABASE_REGION").ok())
            .ok_or_else(|| {
                format!(
                    "No AWS region for KMS key {key_id}: set AWS_REGION, or give the key's full \
                     ARN in OXY_ENCRYPTION_KMS_KEY_ID. Refusing to start without the master key."
                )
            })?;
        let conf = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region))
            .load()
            .await;
        Ok(Self {
            client: aws_sdk_kms::Client::new(&conf),
        })
    }
}

fn sdk_error<E: std::error::Error + 'static>(e: E) -> String {
    aws_sdk_kms::error::DisplayErrorContext(e).to_string()
}

impl Kms for AwsKms {
    async fn encrypt(&self, key_id: &str, plaintext: &[u8]) -> Result<(String, Vec<u8>), String> {
        let out = self
            .client
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
        Ok((out.key_id().unwrap_or(key_id).to_string(), ciphertext))
    }

    async fn decrypt(&self, key_id: &str, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        let out = self
            .client
            .decrypt()
            .key_id(key_id)
            .ciphertext_blob(Blob::new(ciphertext))
            .encryption_context(CONTEXT_KEY, MASTER_KEY_ROW)
            .send()
            .await
            .map_err(sdk_error)?;
        Ok(out
            .plaintext()
            .ok_or("KMS returned no plaintext")?
            .as_ref()
            .to_vec())
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
