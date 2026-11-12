//! Slack's domain half that more than one context needs.
//!
//! - `config`: whether the integration is on, and its app credentials, read
//!   once from the environment.
//! - `installations`: an org's Slack installation, including the bot token
//!   (kept in the org's secrets).
//! - `user_links`: which Slack user a person is, per installation.
//! - `by_email`: link a person to their org's Slack by email, so a
//!   notification can DM someone who has never talked to the bot.
//!
//! Three contexts read these: the Slack bot and its routes (still in
//! `oxy-app`), the Slack DM notification channel (`oxy-api-notifications`)
//! and the anomaly notifier's channel post (`oxy-app`). Two of them are
//! surfaces, and a surface never imports another (S3), so the shared part
//! sits below both. `oxy-app` re-exports each module at its old
//! `integrations::slack` path. See internal-docs/domain-boundaries.md.

pub mod by_email;
pub mod config;
pub mod installations;
pub mod user_links;
