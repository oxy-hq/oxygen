//! The `server_keys.secret` format of the KMS-sealed master key.

use super::MASTER_KEY_ROW;

/// Prefix of the stored row, so a row in another format is refused by name.
pub(super) const ROW_FORMAT: &[u8] = b"oxy-kms-sealed-master-key:v1\0";

/// What `server_keys.secret` holds for [`MASTER_KEY_ROW`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredMasterKey {
    /// The key that sealed it, as KMS reported it (an ARN).
    pub sealed_by: String,
    /// The KMS ciphertext blob.
    pub ciphertext: Vec<u8>,
}

impl StoredMasterKey {
    pub fn to_bytes(&self) -> Vec<u8> {
        let id = self.sealed_by.as_bytes();
        let mut out = Vec::with_capacity(ROW_FORMAT.len() + 2 + id.len() + self.ciphertext.len());
        out.extend_from_slice(ROW_FORMAT);
        out.extend_from_slice(&(id.len() as u16).to_be_bytes());
        out.extend_from_slice(id);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let bad =
            || format!("server_keys row `{MASTER_KEY_ROW}` is not in a format this build reads");
        let rest = bytes.strip_prefix(ROW_FORMAT).ok_or_else(bad)?;
        let (len, rest) = rest.split_first_chunk::<2>().ok_or_else(bad)?;
        let len = u16::from_be_bytes(*len) as usize;
        if rest.len() <= len {
            return Err(bad());
        }
        let (id, ciphertext) = rest.split_at(len);
        Ok(Self {
            sealed_by: String::from_utf8(id.to_vec()).map_err(|_| bad())?,
            ciphertext: ciphertext.to_vec(),
        })
    }
}
