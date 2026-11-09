//! The DNS read a domain's ownership proof needs: the TXT records at one name.
//!
//! Asked over DNS-over-HTTPS (the JSON dialect Cloudflare and Google both
//! serve) rather than the pod's resolver: no resolver crate to add, no cluster
//! DNS cache in front of a record the customer just created, and the answer is
//! the public internet's, which is the one that matters for a public hostname.
//! A TXT record is public data, so asking a public resolver discloses nothing.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

/// The resolvers asked, in order, until one answers. `OXY_CUSTOM_DOMAIN_DOH_URL`
/// replaces the list (one URL) on a deployment that may not reach these.
const DEFAULT_RESOLVERS: &[&str] = &[
    "https://cloudflare-dns.com/dns-query",
    "https://dns.google/resolve",
];

/// RR type 16.
const TXT: u16 = 16;

/// Why a lookup gave no answer. Distinct from "no such record", which is an
/// empty answer.
#[derive(Debug, thiserror::Error)]
#[error("DNS lookup failed: {0}")]
pub struct LookupError(pub String);

/// TXT lookups, behind a trait so verification is testable without a network.
#[async_trait]
pub trait TxtResolver: Send + Sync {
    /// Every TXT value at `name`, each with its character-strings joined.
    /// Empty for NXDOMAIN or a name with no TXT records.
    async fn txt(&self, name: &str) -> Result<Vec<String>, LookupError>;
}

/// [`TxtResolver`] over DNS-over-HTTPS.
pub struct DohResolver {
    client: reqwest::Client,
    resolvers: Vec<String>,
}

impl DohResolver {
    pub fn from_env() -> Self {
        let resolvers = match std::env::var("OXY_CUSTOM_DOMAIN_DOH_URL") {
            Ok(url) if !url.trim().is_empty() => vec![url.trim().to_string()],
            _ => DEFAULT_RESOLVERS.iter().map(|s| s.to_string()).collect(),
        };
        let client = oxy_shared::http::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| oxy_shared::http::client());
        Self { client, resolvers }
    }

    async fn ask(&self, resolver: &str, name: &str) -> Result<Vec<String>, LookupError> {
        let response = self
            .client
            .get(resolver)
            .query(&[("name", name), ("type", "TXT")])
            .header("accept", "application/dns-json")
            .send()
            .await
            .map_err(|e| LookupError(e.to_string()))?;
        if !response.status().is_success() {
            return Err(LookupError(format!(
                "{resolver} answered {}",
                response.status()
            )));
        }
        let body: DohAnswer = response
            .json()
            .await
            .map_err(|e| LookupError(e.to_string()))?;
        body.txt_values()
    }
}

#[async_trait]
impl TxtResolver for DohResolver {
    async fn txt(&self, name: &str) -> Result<Vec<String>, LookupError> {
        let mut last = LookupError("no resolver configured".into());
        for resolver in &self.resolvers {
            match self.ask(resolver, name).await {
                Ok(values) => return Ok(values),
                Err(e) => {
                    tracing::warn!(resolver, name, "custom domain TXT lookup failed: {e}");
                    last = e;
                }
            }
        }
        Err(last)
    }
}

/// The JSON a DoH resolver answers (`application/dns-json`).
#[derive(Debug, Deserialize)]
pub(crate) struct DohAnswer {
    #[serde(rename = "Status")]
    status: u32,
    #[serde(rename = "Answer", default)]
    answer: Vec<DohRecord>,
}

#[derive(Debug, Deserialize)]
struct DohRecord {
    #[serde(rename = "type")]
    rtype: u16,
    data: String,
}

impl DohAnswer {
    /// NOERROR (0) and NXDOMAIN (3) are answers; anything else (SERVFAIL,
    /// REFUSED) is a failed lookup, so a broken resolver never reads as "the
    /// record is missing".
    pub(crate) fn txt_values(&self) -> Result<Vec<String>, LookupError> {
        match self.status {
            0 => Ok(self
                .answer
                .iter()
                .filter(|r| r.rtype == TXT)
                .map(|r| join_character_strings(&r.data))
                .collect()),
            3 => Ok(Vec::new()),
            other => Err(LookupError(format!("resolver returned DNS status {other}"))),
        }
    }
}

/// A TXT record's `data` is one or more quoted character-strings
/// (`"abc" "def"`), split at 255 bytes. The value is their concatenation.
fn join_character_strings(data: &str) -> String {
    let trimmed = data.trim();
    if !trimmed.starts_with('"') {
        return trimmed.to_string();
    }
    let mut out = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    for c in trimmed.chars() {
        match (in_quotes, escaped, c) {
            (true, true, c) => {
                out.push(c);
                escaped = false;
            }
            (true, false, '\\') => escaped = true,
            (_, false, '"') => in_quotes = !in_quotes,
            (true, false, c) => out.push(c),
            (false, _, _) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(json: &str) -> DohAnswer {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn reads_quoted_and_split_txt_values() {
        let a = answer(
            r#"{"Status":0,"Answer":[
                {"name":"x","type":16,"TTL":60,"data":"\"oxygen-verification=abc\""},
                {"name":"x","type":16,"TTL":60,"data":"\"part-one\" \"part-two\""},
                {"name":"x","type":5,"TTL":60,"data":"elsewhere.example."}
            ]}"#,
        );
        assert_eq!(
            a.txt_values().unwrap(),
            vec!["oxygen-verification=abc", "part-onepart-two"]
        );
    }

    #[test]
    fn nxdomain_is_an_empty_answer() {
        assert!(answer(r#"{"Status":3}"#).txt_values().unwrap().is_empty());
    }

    #[test]
    fn servfail_is_a_failed_lookup_not_a_missing_record() {
        assert!(answer(r#"{"Status":2}"#).txt_values().is_err());
    }

    #[test]
    fn unquoted_data_and_escapes() {
        assert_eq!(join_character_strings("plain"), "plain");
        assert_eq!(join_character_strings(r#""a\"b""#), "a\"b");
    }
}
