//! Thin HTTP client for Stripe's REST API. Uses reqwest directly so we avoid
//! bindings churn across async-stripe 0.x ↔ 1.x; every outbound call goes
//! through [`StripeClient::form`] which signs the request with our
//! Restricted API Key and pins `Stripe-Version`.

use std::collections::BTreeMap;

use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::config::StripeConfig;
use crate::errors::BillingError;

const API_BASE: &str = "https://api.stripe.com";
const STRIPE_VERSION: &str = "2026-03-25.dahlia";

#[derive(Clone)]
pub struct StripeClient {
    cfg: StripeConfig,
    http: Client,
}

impl StripeClient {
    pub fn new(cfg: StripeConfig) -> Self {
        Self {
            cfg,
            http: oxy_shared::http::builder()
                .pool_idle_timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("build reqwest client"),
        }
    }

    pub(crate) fn public_url(&self) -> &str {
        &self.cfg.public_url
    }

    pub(crate) fn webhook_signing_secret(&self) -> &str {
        &self.cfg.webhook_signing_secret
    }

    /// Call a Stripe REST endpoint with form-encoded params.
    pub async fn form<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        params: &BTreeMap<String, String>,
        idempotency_key: Option<&str>,
    ) -> Result<T, BillingError> {
        let url = format!("{API_BASE}{path}");
        let mut req = self
            .http
            .request(method, &url)
            .basic_auth(&self.cfg.secret_key, Some(""))
            .header("Stripe-Version", STRIPE_VERSION);
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        let body: Vec<(&String, &String)> = params.iter().collect();
        let resp = req.form(&body).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(resp.json::<T>().await?)
        } else {
            Err(BillingError::Stripe {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    /// Same as `form` but accepts repeated keys (e.g. `allowed_updates[]`)
    /// via an ordered Vec rather than the deduplicating BTreeMap.
    pub async fn form_repeated<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        params: &[(String, String)],
        idempotency_key: Option<&str>,
    ) -> Result<T, BillingError> {
        let url = format!("{API_BASE}{path}");
        let mut req = self
            .http
            .request(method, &url)
            .basic_auth(&self.cfg.secret_key, Some(""))
            .header("Stripe-Version", STRIPE_VERSION);
        if let Some(k) = idempotency_key {
            req = req.header("Idempotency-Key", k);
        }
        let resp = req.form(params).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(resp.json::<T>().await?)
        } else {
            Err(BillingError::Stripe {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, BillingError> {
        let url = format!("{API_BASE}{path}");
        let resp = self
            .http
            .get(&url)
            .basic_auth(&self.cfg.secret_key, Some(""))
            .header("Stripe-Version", STRIPE_VERSION)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            Ok(resp.json::<T>().await?)
        } else if status == StatusCode::NOT_FOUND {
            Err(BillingError::NoSubscription)
        } else {
            Err(BillingError::Stripe {
                status: status.as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }
}
