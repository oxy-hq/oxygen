//! QuickBooks Online integration — OAuth2 authorization-code "Connect" flow.
//!
//! Bring-your-own Intuit app: the user supplies client_id/client_secret and
//! registers this deployment's callback URL in their Intuit app. The
//! authenticated `authorize` handler builds the consent URL; the public
//! `callback` exchanges the code and lands the refresh token in the
//! workspace secret manager (the same store the airway pipeline reads),
//! reusing `SecretsManager::upsert_secret`.

pub mod oauth;
