//! A namespace the overlay stands in: a preview's (live schema `S` ↔
//! `preview_<key>__S`), or a custom-app environment's (the app's own schema ↔
//! its sibling `app_<writer>__<env>`).

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::Refused;
use crate::app_schema::environment_schema;

/// The readable half of a key is capped so the key, and every schema named
/// after it, stays well inside identifier limits.
const SLUG_MAX: usize = 24;
/// Hex characters of `sha256(workspace_id ‖ 0x00 ‖ branch)` in a key.
const HASH_LEN: usize = 6;

/// An Airhouse namespace the overlay redirects writes into. Two kinds, with
/// one set of questions ([`Self::schema_for`], [`Self::owns_schema`]):
///
/// - **A preview's** ([`Self::for_branch`], [`Self::from_key`]). Every schema
///   it may write is `preview_<key>__<live schema>`, and the mapping is
///   one-to-one: a key never contains `__`, and a live schema that contains
///   `__` or starts with `preview_` has no preview schema at all.
/// - **A custom-app environment's** ([`Self::for_environment`]). It stands in
///   for exactly one schema, the app's own `app_<writer>`, with that
///   environment's sibling `app_<writer>__<env>`. Every other schema has no
///   stand-in, so a write to it is refused; a read of it stays live.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewNamespace {
    scheme: Scheme,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Scheme {
    Preview {
        key: String,
    },
    /// Both lowercase: `sibling` is `<live>__<label>`.
    Environment {
        live: String,
        sibling: String,
    },
}

impl PreviewNamespace {
    /// The namespace of `branch`'s preview in `workspace_id`. The same key as
    /// `oxy-app`'s `server::previews::namespace::preview_key`, which every
    /// preview run row carries: `<slug(branch), ≤24 of [a-z0-9_]>_<first 6 hex
    /// of sha256(workspace_id ‖ 0x00 ‖ branch)>`.
    pub fn for_branch(workspace_id: Uuid, branch: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(workspace_id.as_bytes());
        hasher.update([0u8]);
        hasher.update(branch.as_bytes());
        let digest = hex::encode(hasher.finalize());
        Self::preview(format!("{}_{}", slug(branch), &digest[..HASH_LEN]))
    }

    fn preview(key: String) -> Self {
        Self {
            scheme: Scheme::Preview { key },
        }
    }

    /// A namespace from a stored key: `^[a-z0-9_]{1,24}_[0-9a-f]{6}$`, and no
    /// `__` (which [`Self::for_branch`] never produces and which would make a
    /// preview schema name ambiguous to read back).
    pub fn from_key(key: &str) -> Result<Self, Refused> {
        let bad = || Refused(format!("{key:?} is not a preview key"));
        let (slug, hash) = key.rsplit_once('_').ok_or_else(bad)?;
        let slug_ok = (1..=SLUG_MAX).contains(&slug.len())
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        let hash_ok = hash.len() == HASH_LEN
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !slug_ok || !hash_ok || key.contains("__") {
            return Err(bad());
        }
        Ok(Self::preview(key.into()))
    }

    /// The namespace of a custom-app environment: writes to `app_schema` go
    /// to `sibling` and nowhere else. Both lowercase, and `sibling` the name
    /// [`environment_schema`] gives `app_schema` for its label, so it is
    /// `<app_schema>__<label>`: the host builds it that way and a
    /// hand-written one is refused.
    pub fn for_environment(app_schema: &str, sibling: &str) -> Result<Self, Refused> {
        let bad = |why: &str| {
            Refused(format!(
                "{sibling:?} is not an environment sibling of {app_schema:?}: {why}"
            ))
        };
        if app_schema.is_empty() || app_schema.to_ascii_lowercase() != app_schema {
            return Err(bad("both names must be lowercase and non-empty"));
        }
        if sibling == app_schema {
            return Err(bad("the sibling is the app's own schema"));
        }
        let label = sibling
            .strip_prefix(app_schema)
            .and_then(|rest| rest.strip_prefix("__"))
            .ok_or_else(|| bad("a sibling is <app schema>__<label>"))?;
        if environment_schema(app_schema, label).as_deref() != Some(sibling) {
            return Err(bad("it is not the name the app's environment would get"));
        }
        Ok(Self {
            scheme: Scheme::Environment {
                live: app_schema.into(),
                sibling: sibling.into(),
            },
        })
    }

    /// A preview's key, **preview-only**: every previews caller holds a
    /// preview namespace, where this is the key its schemas are named after. For an environment namespace there is no key; it returns
    /// the sibling schema's name, which is what identifies it.
    pub fn key(&self) -> &str {
        match &self.scheme {
            Scheme::Preview { key } => key,
            Scheme::Environment { sibling, .. } => sibling,
        }
    }

    /// `preview_<key>__`, the prefix of every schema this preview owns.
    /// **Preview-only**: an environment owns a single schema and no prefix, so
    /// for one this returns the sibling's name (not a prefix of anything else).
    pub fn prefix(&self) -> String {
        match &self.scheme {
            Scheme::Preview { key } => format!("preview_{key}__"),
            Scheme::Environment { sibling, .. } => sibling.clone(),
        }
    }

    /// The schema standing in for `live_schema`, lowercased the way DuckDB
    /// compares identifiers: a preview's `preview_<key>__S`, or, for an
    /// environment, the sibling when `live_schema` is the app's own schema.
    /// Refused where there is none.
    pub fn schema_for(&self, live_schema: &str) -> Result<String, Refused> {
        let live = live_schema.to_ascii_lowercase();
        match &self.scheme {
            Scheme::Preview { .. } => {
                if let Some(why) = not_a_live_schema(&live) {
                    return Err(Refused(format!(
                        "schema {live_schema:?} {why}, so a preview has no schema standing in \
                         for it"
                    )));
                }
                Ok(format!("{}{live}", self.prefix()))
            }
            Scheme::Environment { live: app, sibling } => {
                if live != *app {
                    return Err(Refused(format!(
                        "schema {live_schema:?} is not {app:?}; an environment overlay stands in \
                         for the app's own schema only"
                    )));
                }
                Ok(sibling.clone())
            }
        }
    }

    /// [`Self::schema_for`] for a **read**. A preview refuses a schema it has
    /// no stand-in for even when reading, as it always has. An environment
    /// reads every other schema live, so there it is `None`, not a refusal.
    pub(super) fn schema_for_read(&self, live_schema: &str) -> Result<Option<String>, Refused> {
        match &self.scheme {
            Scheme::Preview { .. } => self.schema_for(live_schema).map(Some),
            Scheme::Environment { live, .. } if live_schema.eq_ignore_ascii_case(live) => {
                self.schema_for(live_schema).map(Some)
            }
            Scheme::Environment { .. } => Ok(None),
        }
    }

    /// Whether this is an environment's namespace, whose sibling the app's
    /// migrations already made, rather than a preview's, which starts empty.
    pub(super) fn is_environment(&self) -> bool {
        matches!(self.scheme, Scheme::Environment { .. })
    }

    /// Whether `schema` is one of this namespace's: for a preview, its prefix
    /// followed by a name [`Self::schema_for`] would accept; for an
    /// environment, the sibling.
    pub fn owns_schema(&self, schema: &str) -> bool {
        let schema = schema.to_ascii_lowercase();
        match &self.scheme {
            Scheme::Preview { .. } => schema
                .strip_prefix(&self.prefix())
                .is_some_and(|live| not_a_live_schema(live).is_none()),
            Scheme::Environment { sibling, .. } => schema == *sibling,
        }
    }
}

/// Why `live` (lowercased) can't be a live schema a preview stands in for.
fn not_a_live_schema(live: &str) -> Option<&'static str> {
    if live.is_empty() {
        Some("is empty")
    } else if live.starts_with("preview_") {
        Some("starts with preview_, which names preview schemas")
    } else if live.contains("__") {
        Some("contains __, which separates a preview's key from the schema it stands in for")
    } else {
        None
    }
}

/// Lowercase ASCII alphanumerics kept, every other run of characters one `_`,
/// no leading or trailing `_`, at most [`SLUG_MAX`] characters; a branch with
/// nothing usable in it slugs to `preview`.
fn slug(branch: &str) -> String {
    let mut out = String::with_capacity(SLUG_MAX);
    for c in branch.chars().flat_map(char::to_lowercase) {
        if out.len() == SLUG_MAX {
            break;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_end_matches('_');
    if trimmed.is_empty() {
        "preview".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `for_branch(Uuid::from_u128(1), "feat/je-v2")`, computed independently.
    const KEY: &str = "feat_je_v2_92a1b7";

    #[test]
    fn the_key_matches_the_one_oxy_app_gives_every_preview_run() {
        let ws = uuid::Uuid::from_u128(1);
        assert_eq!(PreviewNamespace::for_branch(ws, "feat/je-v2").key(), KEY);
        assert_eq!(
            PreviewNamespace::for_branch(ws, "Feature/ÜBER--long_branch.name/with/many/parts")
                .key(),
            "feature_ber_long_branch_a727b0"
        );
        assert!(
            PreviewNamespace::for_branch(ws, "---")
                .key()
                .starts_with("preview_")
        );
    }

    #[test]
    fn a_key_is_validated_and_names_map_one_to_one() {
        for bad in [
            "",
            "feat",
            "Feat_92a1b7",
            "feat_92A1B7",
            "a__b_92a1b7",
            "feat-x_92a1b7",
        ] {
            assert!(PreviewNamespace::from_key(bad).is_err(), "{bad}");
        }
        let ns = PreviewNamespace::from_key(KEY).unwrap();
        assert_eq!(ns.prefix(), format!("preview_{KEY}__"));
        assert_eq!(
            ns.schema_for("Toast_POS").unwrap(),
            format!("preview_{KEY}__toast_pos")
        );
        for live in ["", "a__b", "preview_x", "PREVIEW_x"] {
            assert!(ns.schema_for(live).is_err(), "{live}");
        }
        assert!(ns.owns_schema(&format!("PREVIEW_{KEY}__toast_pos")));
        for other in [
            "toast_pos",
            &format!("preview_{KEY}__"),
            &format!("preview_{KEY}__a__b"),
        ] {
            assert!(!ns.owns_schema(other), "{other}");
        }
        let other = PreviewNamespace::from_key("feat_je_v3_000000").unwrap();
        assert!(!other.owns_schema(&ns.schema_for("toast_pos").unwrap()));
    }
}
