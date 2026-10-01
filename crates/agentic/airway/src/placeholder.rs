//! Placeholder credentials for building a source connector **offline**.
//!
//! Two callers build a connector from a stored `.airway.yml` without ever
//! running it: the staff contract-policy preview
//! (`admin/airway_config/preview_scan.rs` in `oxy-app`) and the workspace
//! previews' Airway change check, which reads `resources()` and
//! `table_name_mappings()` off the live and the branch spec. Both need the
//! connector to *construct*, and neither may read the secret store. This module
//! is the one place that turns a spec's `*_var` secret references into
//! constructible literals, so the two cannot drift on which keys are
//! credentials.
//!
//! Pure: no I/O, no secret store, no runtime handle.

/// Placeholder written in place of every `*_var` credential reference. Never
/// leaves the process — it exists only so a connector *constructs*.
pub const PLACEHOLDER_SECRET: &str = "oxy-preview-placeholder";

/// `*_var` keys that are **not** substitutable credential references and must
/// survive [`substitute_secret_vars`] untouched.
///
/// `access_token_var` is a *mode selector*, not a credential the factory reads:
/// its presence is what puts a quickbooks source into read-only token custody,
/// and the executor turns it into an `AccessTokenSource` rather than a literal
/// (see `PipelineTaskExecutor::dispatch_airway`). Rewriting it to an
/// `access_token` literal both erases that declaration — dropping the source
/// into the rotating branch, which then fails for want of `client_secret` — and
/// produces a field `QuickBooksParams` rejects under `deny_unknown_fields`.
/// Either way the pipeline could not be evaluated, which is the one outcome
/// an offline build exists to avoid.
pub const NON_CREDENTIAL_VARS: &[&str] = &["access_token_var"];

/// Rewrite every `<field>_var` secret reference into a `<field>` literal
/// holding [`PLACEHOLDER_SECRET`], recursively.
///
/// The run path substitutes *real* secrets before dispatch
/// (`PipelineTaskExecutor::resolve_airway_source_secrets`), and several
/// connector `Params` structs are `deny_unknown_fields` around a required
/// credential — so an unsubstituted spec would not even deserialize, and every
/// toast/quickbooks pipeline would be unevaluable.
///
/// The callers deliberately do **not** read the secret store. A connector's
/// `resources()` and `contracts()` are declared by its code, never by its
/// credential, so the answer is identical either way; resolving for real would
/// turn a staff preview into a credential-presence oracle, and would make every
/// workspace with one missing secret unevaluable — hiding the exact resources
/// the operator asked about. The one thing it costs is that a pipeline whose
/// secret is genuinely absent still evaluates; that is a different problem,
/// with its own error at run time.
///
/// **This is safe only because connector construction performs no I/O.**
/// Every `build_source_connector` arm today deserializes a `Params` struct and
/// hands the fields to a constructor — nothing authenticates, opens a socket,
/// or otherwise looks at whether the credential is real. A future connector
/// that validates its credential *at construction time* would break that
/// assumption in the worst way: an offline build would start making
/// authenticated calls with a fake secret from a staff route, and would report
/// every pipeline of that kind as unevaluable. If an arm ever grows an I/O
/// step in its constructor, this substitution has to be revisited — it is not
/// a detail that can be left to notice itself. It is also what lets a caller
/// build connectors on a blocking thread with no runtime handle.
///
/// Generic on the `_var` suffix rather than a per-kind table, which matches
/// every pair the executor's table lists (`client_secret_var` →
/// `client_secret`, `password_var` → `password`, rest_api's nested
/// `auth.token_var` → `auth.token`, …) without duplicating it here. `<field>`
/// is inserted only when absent, so a spec carrying the literal keeps it.
pub fn substitute_secret_vars(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let var_keys: Vec<String> = map
                .keys()
                .filter(|k| k.strip_suffix("_var").is_some_and(|f| !f.is_empty()))
                .filter(|k| !NON_CREDENTIAL_VARS.contains(&k.as_str()))
                .cloned()
                .collect();
            for var_key in var_keys {
                let field = var_key
                    .strip_suffix("_var")
                    .expect("filtered on the suffix above")
                    .to_string();
                map.remove(&var_key);
                map.entry(field)
                    .or_insert_with(|| serde_json::Value::String(PLACEHOLDER_SECRET.to_string()));
            }
            for nested in map.values_mut() {
                substitute_secret_vars(nested);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                substitute_secret_vars(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat and nested `*_var` references both become literals, so a connector
    /// whose `Params` is `deny_unknown_fields` around a required credential
    /// still constructs. Without this the toast/quickbooks arms would reject
    /// every real pipeline.
    #[test]
    fn secret_var_references_become_placeholder_literals() {
        let mut config = serde_json::json!({
            "client_id": "id-123",
            "client_secret_var": "TOAST_SECRET",
            "restaurant_guids": ["g-1"],
            "auth": { "token_var": "REST_TOKEN" },
            "endpoints": [{ "name": "charges", "key_var": "NESTED_KEY" }],
        });
        substitute_secret_vars(&mut config);

        assert!(
            config.get("client_secret_var").is_none(),
            "`_var` is stripped"
        );
        assert_eq!(
            config["client_secret"], PLACEHOLDER_SECRET,
            "the literal field it names is filled in"
        );
        assert!(config["auth"]["token"].is_string(), "nested objects too");
        assert!(
            config["endpoints"][0]["key"].is_string(),
            "and objects inside arrays"
        );
        assert_eq!(
            config["client_id"], "id-123",
            "non-credential fields are untouched"
        );
    }

    /// A spec that already carries the literal keeps it — the placeholder
    /// fills a gap, it does not overwrite an author's value.
    #[test]
    fn an_explicit_literal_survives_substitution() {
        let mut config = serde_json::json!({
            "client_secret": "literal-in-yaml",
            "client_secret_var": "TOAST_SECRET",
        });
        substitute_secret_vars(&mut config);
        assert_eq!(config["client_secret"], "literal-in-yaml");
        assert!(config.get("client_secret_var").is_none());
    }

    /// `access_token_var` is a token-custody *mode selector*, not a credential
    /// the factory reads — the executor turns it into an `AccessTokenSource`,
    /// never a literal. Substituting it would both erase the read-only
    /// declaration (so the source falls into the rotating branch and fails for
    /// want of `client_secret`) and produce a field `QuickBooksParams` rejects
    /// under `deny_unknown_fields`.
    #[test]
    fn access_token_var_survives_substitution() {
        let mut config = serde_json::json!({
            "client_id": "id-123",
            "realm_id": "9341456441393444",
            "access_token_var": "apps/app-id/QB_ACCESS_TOKEN",
        });
        substitute_secret_vars(&mut config);

        assert_eq!(
            config["access_token_var"], "apps/app-id/QB_ACCESS_TOKEN",
            "the mode selector must reach the factory intact"
        );
        assert!(
            config.get("access_token").is_none(),
            "and must not be rewritten into a literal the params struct rejects"
        );
    }

    /// The exclusion is by exact key, so a genuine credential whose name
    /// merely ends the same way is still substituted.
    #[test]
    fn other_token_vars_are_still_substituted() {
        let mut config = serde_json::json!({
            "refresh_token_var": "QB_REFRESH_TOKEN",
            "auth": { "token_var": "REST_TOKEN" },
        });
        substitute_secret_vars(&mut config);

        assert!(config.get("refresh_token_var").is_none());
        assert!(config["refresh_token"].is_string());
        assert!(config["auth"]["token"].is_string());
    }
}
