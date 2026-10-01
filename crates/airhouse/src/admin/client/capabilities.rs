//! `GET /admin/v1/capabilities`: features a platform confirms before relying
//! on them. An Airhouse older than the endpoint answers 404.

use reqwest::StatusCode;
use serde::Deserialize;

use super::AirhouseAdminClient;
use crate::admin::error::AirhouseError;

/// What this Airhouse deployment can do, as far as Oxy asks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Capabilities {
    /// A mint's `write_schemas` confines the Writer and is echoed back
    /// (airhouse 0.1.49 and later).
    #[serde(rename = "mint.write_schemas", default)]
    pub mint_write_schemas: bool,
}

impl AirhouseAdminClient {
    /// Ask the deployment what it supports, with the admin token. A 404 is an
    /// Airhouse that predates the endpoint, so it supports none of it.
    pub async fn capabilities(&self) -> Result<Capabilities, AirhouseError> {
        let resp = self
            .client
            .get(self.url("/capabilities"))
            .bearer_auth(&self.token)
            .send()
            .await?;
        match resp.status() {
            StatusCode::OK => Ok(resp.json::<Capabilities>().await?),
            StatusCode::NOT_FOUND => Ok(Capabilities::default()),
            StatusCode::UNAUTHORIZED => Err(AirhouseError::Unauthorized(resp.text().await?)),
            StatusCode::TOO_MANY_REQUESTS => Err(AirhouseError::RateLimited(resp.text().await?)),
            s => Err(AirhouseError::Provisioning(format!(
                "airhouse /capabilities returned unexpected status {s}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn capabilities_are_read_and_a_404_supports_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/admin/v1/capabilities"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"mint.write_schemas": true})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = AirhouseAdminClient::new(server.uri(), "tok");
        assert!(client.capabilities().await.unwrap().mint_write_schemas);

        let old = MockServer::start().await;
        let client = AirhouseAdminClient::new(old.uri(), "tok");
        assert_eq!(
            client.capabilities().await.unwrap(),
            Capabilities::default()
        );
    }
}
