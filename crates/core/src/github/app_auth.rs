use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use oxy_shared::errors::OxyError;
use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use std::env;
use tracing::debug;

#[derive(Debug, Serialize, Deserialize)]
struct AppTokenClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: String,
}

pub struct GitHubAppAuth {
    app_id: String,
    private_key: String,
    client: Client,
    base_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GitHubAccount {
    pub(crate) login: String,
    #[serde(rename = "type")]
    pub(crate) account_type: String,
    pub(crate) id: i64,
    // Other fields available but not needed for our use case
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GitHubInstallationResponse {
    pub(crate) id: i64,
    pub(crate) account: GitHubAccount,
    pub(crate) app_slug: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubInstallation {
    pub id: i64,
    pub owner_type: String,
    pub slug: String,
    pub name: String,
}

impl GitHubAppAuth {
    pub fn new(app_id: String, private_key: String) -> Result<Self, OxyError> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Oxy-GitHub-App/1.0"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github.v3+json"),
        );

        let client = oxy_shared::http::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| OxyError::RuntimeError(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            app_id,
            private_key,
            client,
            base_url: "https://api.github.com".to_string(),
        })
    }

    pub fn from_env() -> Result<Self, OxyError> {
        let app_id = env::var("GITHUB_APP_ID").map_err(|_| {
            OxyError::ConfigurationError("GitHub App ID not configured in environment".to_string())
        })?;

        let private_key = env::var("GITHUB_APP_PRIVATE_KEY").map_err(|_| {
            OxyError::ConfigurationError(
                "GitHub App private key not configured in environment".to_string(),
            )
        })?;

        Self::new(app_id, private_key)
    }

    pub async fn list_installations(&self) -> Result<Vec<GitHubInstallation>, OxyError> {
        let jwt = self.generate_jwt()?;

        let url = format!("{}/app/installations", self.base_url);
        let response = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", jwt))
            .send()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("Failed to list app installations: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(OxyError::RuntimeError(format!(
                "GitHub API error: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let installations: Vec<GitHubInstallationResponse> =
            response.json().await.map_err(|e| {
                OxyError::RuntimeError(format!("Failed to parse installations list: {e}"))
            })?;

        Ok(installations
            .into_iter()
            .map(|r| GitHubInstallation {
                id: r.id,
                owner_type: r.account.account_type,
                slug: r.account.login.clone(),
                name: r.account.login,
            })
            .collect())
    }

    pub async fn get_installation_info(
        &self,
        installation_id: &str,
    ) -> Result<GitHubInstallation, OxyError> {
        let jwt = self.generate_jwt()?;

        let url = format!("{}/app/installations/{}", self.base_url, installation_id);
        let response = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", jwt))
            .send()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Failed to get installation info: {e}")))?;

        if !response.status().is_success() {
            return Err(OxyError::RuntimeError(format!(
                "GitHub API error: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let installation_response: GitHubInstallationResponse =
            response.json().await.map_err(|e| {
                OxyError::RuntimeError(format!("Failed to parse installation info: {e}"))
            })?;

        let installation = GitHubInstallation {
            id: installation_response.id,
            owner_type: installation_response.account.account_type,
            slug: installation_response.account.login.clone(),
            name: installation_response.account.login,
        };

        Ok(installation)
    }

    pub fn generate_jwt(&self) -> Result<String, OxyError> {
        let now = Utc::now();
        let iat = now.timestamp();
        let exp = (now + Duration::minutes(10)).timestamp();

        let claims = AppTokenClaims {
            iat,
            exp,
            iss: self.app_id.clone(),
        };

        let header = Header::new(Algorithm::RS256);
        let key = EncodingKey::from_rsa_pem(self.private_key.as_bytes())
            .map_err(|e| OxyError::RuntimeError(format!("Invalid private key: {e}")))?;

        encode(&header, &claims, &key)
            .map_err(|e| OxyError::RuntimeError(format!("Failed to generate JWT: {e}")))
    }

    pub async fn get_installation_token(&self, installation_id: &str) -> Result<String, OxyError> {
        debug!(
            "Getting installation token for installation ID: {}",
            installation_id
        );
        let jwt = self.generate_jwt()?;

        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.base_url, installation_id
        );

        let response = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {}", jwt))
            .send()
            .await
            .map_err(|e| {
                OxyError::RuntimeError(format!("Failed to get installation token: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(OxyError::RuntimeError(format!(
                "GitHub API error: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let token_response: InstallationTokenResponse = response
            .json()
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Failed to parse token response: {e}")))?;

        debug!(
            "Successfully obtained installation token, expires at: {}",
            token_response.expires_at
        );
        Ok(token_response.token)
    }
}
