//! The schemas a custom app owns in its workspace's Airhouse, and how a
//! non-production environment's copy of one is named.
//!
//! An app writes its facts to `app_<writer>`, the writer derived from its slug
//! (`oxy_oltp::schema::app_writer_name`). A non-production environment writes
//! to a **sibling** of that schema, `app_<writer>__<label>` — staging's is
//! `app_<writer>__staging`, and a sandbox `dev-<handle>`'s is
//! `app_<writer>__dev_<handle>`, the handle's hyphens written as underscores
//! (environments design §11, option (a); `internal-docs/custom-app-sandboxes.md`).
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
//! only a name [`environment_schema`] gives for a known environment label
//! (`staging`, or a sandbox's `dev_<handle>`), and
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

/// The sibling of `app_schema` for the environment whose schema label is
/// `environment`: `app_x` and `staging` give `app_x__staging`; `app_x` and
/// `dev_a1` give `app_x__dev_a1`.
///
/// `environment` is a schema **label**, not an environment name
/// (`AppEnvironment::schema_label`): `[a-z][a-z0-9]*(_[a-z0-9]+)*`. A sandbox's
/// `dev-<handle>` is passed as `dev_<handle>` with each hyphen as an
/// underscore, because a hyphen would need quoting.
///
/// `None` — and the caller holds the write rather than guess a home — when
/// the pair cannot name a sibling that reads back as one: `app_schema` is not
/// an app's (`app_…`) or already holds the separator (a legacy slug with `--`
/// derives `app_a__b`, which would read as app `a`'s `b` environment),
/// `environment` is not a label (a hyphen, an upper-case letter, a leading,
/// trailing or doubled underscore — the last would put a second separator in
/// the name), or the sibling would be longer than [`MAX_SCHEMA_LEN`] bytes.
pub fn environment_schema(app_schema: &str, environment: &str) -> Option<String> {
    let plain_app = app_schema.starts_with(APP_SCHEMA_PREFIX)
        && app_schema.len() > APP_SCHEMA_PREFIX.len()
        && !app_schema.contains(ENVIRONMENT_SEPARATOR)
        && app_schema
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let sibling = format!("{app_schema}{ENVIRONMENT_SEPARATOR}{environment}");
    (plain_app && is_schema_label(environment) && sibling.len() <= MAX_SCHEMA_LEN)
        .then_some(sibling)
}

/// `[a-z][a-z0-9]*(_[a-z0-9]+)*`: lowercase letters, digits and single inner
/// underscores, starting with a letter.
fn is_schema_label(label: &str) -> bool {
    label.starts_with(|c: char| c.is_ascii_lowercase())
        && !label.ends_with('_')
        && !label.contains(ENVIRONMENT_SEPARATOR)
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Staging's label, and the prefix of a sandbox's.
const STAGING_LABEL: &str = "staging";
const DEV_LABEL_PREFIX: &str = "dev_";

/// The longest sandbox handle (`oxy_app_core::custom_app_environment::
/// DEV_HANDLE_MAX_LEN`; this crate does not depend on that one, and a test in
/// `oxy-app` pins the two rules together).
const DEV_HANDLE_MAX_LEN: usize = 12;

/// Whether `label` is one an environment that has a sibling carries:
/// `staging`, or `dev_<handle>` where the handle, with each underscore read
/// back as a hyphen, is a valid sandbox handle — 1 to 12 characters of
/// lowercase letters, digits and single inner separators.
fn is_known_environment_label(label: &str) -> bool {
    if label == STAGING_LABEL {
        return true;
    }
    label.strip_prefix(DEV_LABEL_PREFIX).is_some_and(|handle| {
        !handle.is_empty()
            && handle.len() <= DEV_HANDLE_MAX_LEN
            && !handle.starts_with('_')
            && !handle.ends_with('_')
            && !handle.contains(ENVIRONMENT_SEPARATOR)
            && handle
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    })
}

/// Whether `schema` is an app's non-production sibling — *exactly* the name
/// [`environment_schema`] gives some app for staging or for a sandbox,
/// compared without case. Hidden from every schema listing, so its facts never
/// appear beside production's, and refused to another app's reads
/// (`sql_rules`). A legacy `--`-slug production schema (`app_a__b`) has the
/// `app_*__*` shape but is nobody's sibling, so it lists.
///
/// The name is split at its **first** separator: an app schema never holds
/// one, so whatever follows is the label, and a label holding a second
/// separator is not one.
pub fn is_environment_schema(schema: &str) -> bool {
    let lowered = schema.to_ascii_lowercase();
    lowered
        .split_once(ENVIRONMENT_SEPARATOR)
        .is_some_and(|(app, label)| {
            is_known_environment_label(label)
                && environment_schema(app, label).as_deref() == Some(lowered.as_str())
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

    /// A sandbox's label is `dev_<handle>` with the handle's hyphens as
    /// underscores; its sibling is named and hidden like staging's.
    #[test]
    fn a_dev_label_names_a_sibling_within_the_identifier_limit() {
        assert_eq!(
            environment_schema("app_store_ops", "dev_a1_b2").as_deref(),
            Some("app_store_ops__dev_a1_b2")
        );
        // 4 + 41 + 2 + 16 = 63: the longest writer a 12-character handle fits.
        let writer = format!("app_{}", "a".repeat(41));
        let sibling = environment_schema(&writer, "dev_abcdefghijkl").expect("63 bytes fits");
        assert_eq!(sibling.len(), MAX_SCHEMA_LEN);
        assert!(is_environment_schema(&sibling));
    }

    #[test]
    fn a_writer_too_long_for_a_dev_label_names_no_sibling() {
        let writer = format!("app_{}", "a".repeat(42));
        assert_eq!(environment_schema(&writer, "dev_abcdefghijkl"), None);
        // …while staging's shorter label still fits the same writer.
        assert!(environment_schema(&writer, "staging").is_some());
    }

    /// A label is `[a-z][a-z0-9]*(_[a-z0-9]+)*`: single underscores only, and
    /// never a trailing one — either would let two names meet.
    #[test]
    fn a_label_holds_single_inner_underscores_only() {
        for label in [
            "dev_", "dev__a", "_dev", "dev_a_", "dev_a__b", "dev-a", "Dev_a",
        ] {
            assert_eq!(environment_schema("app_x", label), None, "{label}");
        }
        assert!(environment_schema("app_x", "dev_a").is_some());
    }

    #[test]
    fn a_sandbox_sibling_is_hidden_and_a_lookalike_is_not() {
        for hidden in [
            "app_x__dev_a1_b2",
            "APP_X__DEV_A1_B2",
            "app_x__dev_a",
            "app_store_ops__dev_abcdefghijkl",
        ] {
            assert!(is_environment_schema(hidden), "{hidden}");
        }
        for listed in [
            "app_a__b",
            "app_x__prod",
            "app_x__dev_",
            "app_x__dev__a",
            // `dev` alone is no sandbox: the handle is empty.
            "app_x__dev",
            // A 13-character handle is not one `AppEnvironment::parse` accepts.
            "app_x__dev_abcdefghijklm",
            // Not built on a `__` name, as staging's is not.
            "app_a__b__dev_x",
            "app_x__dev_a__staging",
            "app___dev_a",
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
