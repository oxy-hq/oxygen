//! A preview's key: one stable, schema-safe name per `(workspace, branch)`.
//!
//! `preview_key = <slug(branch), ≤24 chars of [a-z0-9_]>_<first 6 hex of
//! sha256(workspace_id ‖ 0x00 ‖ branch)>`. The slug keeps it readable
//! (`feat_je_v2_91ab0e`); the hash keeps two branches that slug alike (`feat/x`
//! and `feat-x`), or the same branch in two workspaces, apart. Every preview run
//! row carries it, and the preview's Airhouse schemas are named after it
//! (`preview_<key>__<live schema>`), which is why only `[a-z0-9_]` survives.
//!
//! One implementation: [`airhouse::preview_sql::PreviewNamespace`], which also
//! owns the schema-name mapping the SQL rewrite, its verifier and the TTL drop
//! enforce. This module only names it for the rest of `server::previews`.

pub use airhouse::preview_sql::PreviewNamespace;
use uuid::Uuid;

/// The key every preview run of `branch` in `workspace_id` carries.
pub fn preview_key(workspace_id: Uuid, branch: &str) -> String {
    PreviewNamespace::for_branch(workspace_id, branch)
        .key()
        .to_string()
}

/// The namespace a stored preview run writes into. `Err` when the row's key is
/// not one [`preview_key`] could have produced (a hand-edited row): nothing is
/// created or dropped under a key that fails validation.
pub fn for_preview(
    run: &entity::workspace_preview_runs::Model,
) -> Result<PreviewNamespace, airhouse::preview_sql::Refused> {
    PreviewNamespace::from_key(&run.preview_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Readable slug (≤24) + `_` + six hex.
    const KEY_MAX: usize = 24 + 1 + 6;

    fn ws(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// Golden values, computed independently of both implementations (Python
    /// `hashlib`) back when the key had two: every `workspace_preview_runs` row
    /// already written carries a key from this function, so it may never move.
    #[test]
    fn the_key_is_pinned_to_golden_values() {
        assert_eq!(preview_key(ws(1), "feat/je-v2"), "feat_je_v2_92a1b7");
        assert_eq!(
            preview_key(ws(1), "Feature/ÜBER--long_branch.name/with/many/parts"),
            "feature_ber_long_branch_a727b0"
        );
    }

    #[test]
    fn a_key_is_a_readable_slug_and_a_six_hex_suffix() {
        let key = preview_key(ws(1), "feat/je-v2");
        let (slug, hash) = key.rsplit_once('_').unwrap();
        assert_eq!(slug, "feat_je_v2");
        assert_eq!(hash.len(), 6);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_key_is_stable_and_schema_safe() {
        let key = preview_key(ws(7), "Feature/ÜBER--long_branch.name/with/many/parts");
        assert_eq!(
            key,
            preview_key(ws(7), "Feature/ÜBER--long_branch.name/with/many/parts")
        );
        assert!(key.len() <= KEY_MAX, "{key}");
        assert!(
            key.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{key}"
        );
        assert!(!key.starts_with('_'), "{key}");
    }

    #[test]
    fn branches_that_slug_alike_and_other_workspaces_get_other_keys() {
        assert_ne!(preview_key(ws(1), "feat/x"), preview_key(ws(1), "feat-x"));
        assert_ne!(preview_key(ws(1), "feat/x"), preview_key(ws(2), "feat/x"));
    }

    #[test]
    fn a_branch_with_nothing_sluggable_still_gets_a_key() {
        assert!(preview_key(ws(1), "---").starts_with("preview_"));
    }

    /// Every key this module hands out passes the validation the registry and
    /// the drop apply to a stored key.
    #[test]
    fn every_key_is_one_a_stored_row_validates() {
        for branch in ["feat/je-v2", "---", "a", "Feature/ÜBER--long_branch.name"] {
            let key = preview_key(ws(3), branch);
            assert_eq!(PreviewNamespace::from_key(&key).unwrap().key(), key);
        }
    }
}
