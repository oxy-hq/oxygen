//! The schemas a custom app owns in its workspace's Airhouse, and how a
//! non-production environment's copy of one is named.
//!
//! An app writes its facts to `app_<writer>`, the writer derived from its slug
//! (`oxy_oltp::schema::app_writer_name`). A non-production environment writes
//! to a **sibling** of that schema, `app_<writer>__<environment>` — staging's
//! is `app_<writer>__staging` (environments design §11, option (a)).
//!
//! The separator is a double underscore because nothing else can produce one.
//! A slug is lowercase letters, digits and single hyphens, and its writer maps
//! each hyphen to one underscore, so no writer holds `__`. A single underscore
//! would collide: `app_foo_staging` is the production schema of an app whose
//! slug is `foo-staging`, and app `foo`'s staging writes would land in it.
//!
//! A new app's schema never holds `__`, but a legacy slug with `--` derives one
//! (`app_a__b`) — a production schema, not app `a`'s `b` environment. So
//! [`is_environment_schema`] does not match the `app_*__*` shape: it matches
//! only a name [`environment_schema`] gives for a known environment label, and
//! it is how every schema listing hides siblings — staging facts never show
//! beside production's where the analytics agent, the IDE or the semantic
//! model list what Airhouse holds, while a legacy schema still lists.
//! Workspace previews already give `__` names no stand-in (`preview_sql`: a
//! live schema containing `__` has none).

/// Between an app's schema and the environment its sibling belongs to.
pub const ENVIRONMENT_SEPARATOR: &str = "__";

/// The prefix of every schema a custom app owns (`oxy_oltp::schema`).
const APP_SCHEMA_PREFIX: &str = "app_";

/// The longest sibling name: the Postgres identifier limit Airhouse's wire
/// protocol and its catalog inherit. Longer, and a sibling names none — its
/// writes hold rather than risk a truncated name meeting another schema.
pub const MAX_SCHEMA_LEN: usize = 63;

/// The sibling of `app_schema` for `environment`: `app_x` and `staging` give
/// `app_x__staging`.
///
/// `None` — and the caller holds the write rather than guess a home — when
/// the pair cannot name a sibling that reads back as one: `app_schema` is not
/// an app's (`app_…`) or already holds the separator (a legacy slug with `--`
/// derives `app_a__b`, which would read as app `a`'s `b` environment),
/// `environment` is not a plain label (`[a-z][a-z0-9]*`: a dev slot's
/// `dev-<handle>` would need quoting), or the sibling would be longer than
/// [`MAX_SCHEMA_LEN`] bytes.
pub fn environment_schema(app_schema: &str, environment: &str) -> Option<String> {
    let plain_app = app_schema.starts_with(APP_SCHEMA_PREFIX)
        && app_schema.len() > APP_SCHEMA_PREFIX.len()
        && !app_schema.contains(ENVIRONMENT_SEPARATOR)
        && app_schema
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let mut label = environment.chars();
    let plain_env = matches!(label.next(), Some(c) if c.is_ascii_lowercase())
        && label.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let sibling = format!("{app_schema}{ENVIRONMENT_SEPARATOR}{environment}");
    (plain_app && plain_env && sibling.len() <= MAX_SCHEMA_LEN).then_some(sibling)
}

/// The environments that have a sibling: the labels [`is_environment_schema`]
/// recognizes. A dev slot's `dev-<handle>` never forms one —
/// [`environment_schema`] refuses a label holding a hyphen — so only
/// `staging` matters today; a future plain-label environment is one addition
/// here.
const KNOWN_ENVIRONMENT_LABELS: [&str; 1] = ["staging"];

/// Whether `schema` is an app's non-production sibling — *exactly* the name
/// [`environment_schema`] gives some app for one of
/// [`KNOWN_ENVIRONMENT_LABELS`], compared without case. Hidden from every
/// schema listing, so its facts never appear beside production's, and refused
/// to another app's reads (`sql_rules`). A legacy `--`-slug production schema
/// (`app_a__b`) has the `app_*__*` shape but is nobody's sibling, so it lists.
pub fn is_environment_schema(schema: &str) -> bool {
    let lowered = schema.to_ascii_lowercase();
    KNOWN_ENVIRONMENT_LABELS.iter().any(|label| {
        let suffix = format!("{ENVIRONMENT_SEPARATOR}{label}");
        lowered
            .strip_suffix(suffix.as_str())
            .is_some_and(|app| environment_schema(app, label).as_deref() == Some(lowered.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sibling_is_the_app_schema_and_the_environment() {
        assert_eq!(
            environment_schema("app_store_ops", "staging").as_deref(),
            Some("app_store_ops__staging")
        );
        assert!(is_environment_schema(
            &environment_schema("app_store_ops", "staging").unwrap()
        ));
    }

    /// A pair that could not be told apart from another app's schema, or that
    /// would need quoting, names no sibling: the host then holds the write.
    #[test]
    fn no_sibling_for_a_name_that_would_not_read_back_as_one() {
        for (app, env) in [
            ("app_a__b", "staging"),
            ("store_ops", "staging"),
            ("app_", "staging"),
            ("app_store_ops", "dev-luong"),
            ("app_store_ops", ""),
            ("app_store_ops", "Staging"),
            ("APP_STORE_OPS", "staging"),
            // 4 + 50 + 2 + 7 = 63 fits; one more character does not.
            (format!("app_{}", "a".repeat(51)).as_str(), "staging"),
        ] {
            assert_eq!(environment_schema(app, env), None, "{app} / {env}");
        }
    }

    #[test]
    fn a_sibling_fits_the_identifier_limit() {
        let longest = format!("app_{}", "a".repeat(50));
        let sibling = environment_schema(&longest, "staging").expect("63 bytes fits");
        assert_eq!(sibling.len(), MAX_SCHEMA_LEN);
    }

    #[test]
    fn only_a_sibling_is_hidden() {
        assert!(is_environment_schema("app_store_ops__staging"));
        assert!(is_environment_schema("APP_STORE_OPS__STAGING"));
        assert!(is_environment_schema("app_x__staging"));
        for listed in [
            "app_store_ops",
            // An app whose slug is `store-ops-staging` owns this one.
            "app_store_ops_staging",
            "raw_toast",
            "main",
            "preview_ab12__app_store_ops",
            // A legacy `a--b` slug's own production schema, not app `a`'s
            // `b` environment.
            "app_a__b",
            // …nor app `a__b`'s staging: no sibling is built on a `__` name.
            "app_a__b__staging",
            // No known label, and none produces a hyphen.
            "app_store_ops__dev-luong",
            "app_store_ops__prod",
            "app___staging",
        ] {
            assert!(!is_environment_schema(listed), "{listed}");
        }
    }

    /// No slug an app may have derives a writer holding the separator, so no
    /// production schema reads as a sibling. Slugs: lowercase letters, digits
    /// and single hyphens (`admin::apps::is_valid_slug`).
    #[test]
    fn no_production_schema_holds_the_separator() {
        for slug in ["a", "store-ops", "store-ops-staging", "x1-y2-z3"] {
            let writer = slug.replace('-', "_");
            let schema = format!("{APP_SCHEMA_PREFIX}{writer}");
            assert!(!is_environment_schema(&schema), "{schema}");
        }
    }
}
