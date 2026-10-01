//! A preview's namespace: live schema `S` ↔ `preview_<key>__S`.

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::Refused;

/// The readable half of a key is capped so the key, and every schema named
/// after it, stays well inside identifier limits.
const SLUG_MAX: usize = 24;
/// Hex characters of `sha256(workspace_id ‖ 0x00 ‖ branch)` in a key.
const HASH_LEN: usize = 6;

/// One preview's Airhouse namespace. Every schema it may write is
/// `preview_<key>__<live schema>`, and the mapping is one-to-one: a key never
/// contains `__`, and a live schema that contains `__` or starts with
/// `preview_` has no preview schema at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewNamespace {
    key: String,
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
        Self {
            key: format!("{}_{}", slug(branch), &digest[..HASH_LEN]),
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
        Ok(Self { key: key.into() })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// `preview_<key>__`, the prefix of every schema this preview owns.
    pub fn prefix(&self) -> String {
        format!("preview_{}__", self.key)
    }

    /// The preview schema standing in for `live_schema`, lowercased the way
    /// DuckDB compares identifiers.
    pub fn schema_for(&self, live_schema: &str) -> Result<String, Refused> {
        let live = live_schema.to_ascii_lowercase();
        if let Some(why) = not_a_live_schema(&live) {
            return Err(Refused(format!(
                "schema {live_schema:?} {why}, so a preview has no schema standing in for it"
            )));
        }
        Ok(format!("{}{live}", self.prefix()))
    }

    /// Whether `schema` is one of this preview's: its prefix followed by a
    /// name [`Self::schema_for`] would accept.
    pub fn owns_schema(&self, schema: &str) -> bool {
        let schema = schema.to_ascii_lowercase();
        schema
            .strip_prefix(&self.prefix())
            .is_some_and(|live| not_a_live_schema(live).is_none())
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
