//! What a claim tells the runner beside the attempt itself: which screens the
//! build declares, what production serves now, and what one attempt may keep.
//!
//! **Screens.** A build's screens are `checks.screens` of its manifest
//! (`app_builds.manifest_json`), read under the rule the SDK publishes
//! (`sdk/typescript` `manifest-rules.ts`: `SCREEN_PATH_PATTERN`,
//! `SCREEN_PATH_MAX_LEN`, `SCREENS_MAX`): an app-relative path with no leading
//! slash, no scheme, no `..`, no query and no hash, at most 256 characters
//! (`""` is the root); each listed once, at most 30. **An entry that does not
//! pass is dropped, never a refused claim**: a bundle published straight
//! through the API skips the SDK's validation, and a build's checks must still
//! run. `null` when the manifest declares none: the runner then reads the
//! screens off the root page's links.
//!
//! **Production.** For a staging attempt, the build production serves now, so
//! the runner can show what changed against it: `null` when production serves
//! nothing, or serves this same build (there is nothing to compare, and the
//! runner reports `changes` as `skipped`). Both of the build's ids are named:
//! `id` is the row id, which is what a page reports as
//! `window.__OXY_APP__.buildId`; `build_id` is the publish id a person reads.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::super::files::{MAX_FILE_BYTES, MAX_FILES};
use crate::custom_apps_environments;

/// The most screens one attempt opens.
pub const MAX_SCREENS: usize = 30;
/// The longest declared screen path.
pub const MAX_SCREEN_CHARS: usize = 256;

/// What one attempt may keep and open, as the claim says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// The most screens opened.
    pub screens: usize,
    /// The largest file kept, in bytes.
    pub file_bytes: usize,
    /// The most files kept for the attempt.
    pub files: usize,
}

/// The limits of every attempt.
pub const LIMITS: Limits = Limits {
    screens: MAX_SCREENS,
    file_bytes: MAX_FILE_BYTES,
    files: MAX_FILES,
};

/// Whether `entry` starts with a URL scheme (`https:`, `mailto:`, `x-y.z:`).
fn has_scheme(entry: &str) -> bool {
    let Some((scheme, _)) = entry.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Whether `entry` is a screen the SDK's rule admits:
/// `^(?!\/|[A-Za-z][A-Za-z0-9+.-]*:)(?![^?]*\.\.)[^?#]*(?:\?[^#\s]+)?$`, at
/// most [`MAX_SCREEN_CHARS`] characters. `""` is the root. A query is part of
/// the screen, for an app that picks its screen by one (`?module=waste`,
/// `stores?tab=open`); a hash never is.
pub fn is_screen_path(entry: &str) -> bool {
    let (path, query) = match entry.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (entry, None),
    };
    entry.chars().count() <= MAX_SCREEN_CHARS
        && !path.starts_with('/')
        && !has_scheme(entry)
        && !path.contains("..")
        && !path.contains('#')
        && query.is_none_or(is_screen_query)
}

/// A screen's query: something, with no hash and no whitespace in it.
fn is_screen_query(query: &str) -> bool {
    !query.is_empty() && !query.contains(|c: char| c == '#' || c.is_whitespace())
}

/// The screens `manifest` declares (`checks.screens`), each one that passes
/// [`is_screen_path`], once, in the manifest's order, at most [`MAX_SCREENS`].
/// `None` when it declares none: no manifest, no `checks`, or a `screens`
/// that is not a list.
pub fn declared_screens(manifest: Option<&Value>) -> Option<Vec<String>> {
    let declared = manifest?.get("checks")?.get("screens")?.as_array()?;
    let mut screens: Vec<String> = Vec::new();
    for entry in declared.iter().filter_map(Value::as_str) {
        if screens.len() == MAX_SCREENS {
            break;
        }
        if is_screen_path(entry) && !screens.iter().any(|kept| kept == entry) {
            screens.push(entry.to_string());
        }
    }
    Some(screens)
}

/// A build production serves, by both of its ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProductionRef {
    /// `app_builds.id`: what production's page reports as its build.
    pub id: Uuid,
    /// The publish id.
    pub build_id: String,
}

/// The build production serves now, for a staging attempt of `build`: `None`
/// when it serves nothing, or this same build.
pub async fn production_of<C: ConnectionTrait>(
    conn: &C,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<Option<ProductionRef>, DbErr> {
    let production = AppEnvironment::Production;
    let serving = custom_apps_environments::pointer(conn, app.id, &production).await?;
    let Some(serving) = serving.filter(|pk| *pk != build.id) else {
        return Ok(None);
    };
    Ok(app_builds::Entity::find_by_id(serving)
        .one(conn)
        .await?
        .filter(|served| served.app_id == app.id)
        .map(|served| ProductionRef {
            id: served.id,
            build_id: served.build_id,
        }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_screen_is_an_app_relative_path_with_no_slash_scheme_dots_query_or_hash() {
        for good in [
            "",
            "stores",
            "reorder/plans",
            "stores/42/edit",
            "a.b/c-d_e",
            "with space",
            "caf\u{e9}",
            "time:12",
            &"a".repeat(MAX_SCREEN_CHARS),
        ] {
            // `time:12` has a scheme by the SDK's rule and is refused below;
            // every other entry here passes.
            assert_eq!(is_screen_path(good), good != "time:12", "{good:?}");
        }
        for bad in [
            "/stores",
            "//evil.example.com",
            "https://evil.example.com/x",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "x-y.z+1:rest",
            "../admin",
            "stores/../../x",
            "a..b",
            "stores#top",
            &"a".repeat(MAX_SCREEN_CHARS + 1),
        ] {
            assert!(!is_screen_path(bad), "{bad:?}");
        }
        // A query is the screen's own, for an app that picks its screen by one.
        for good in [
            "?module=waste",
            "stores?week=47",
            "?module=temps&view=week",
            "?range=1..5",
            "?at=10:30",
            "?next=/stores",
        ] {
            assert!(is_screen_path(good), "{good:?}");
        }
        for bad in [
            "?",
            "stores?",
            "stores?a b",
            "?a=1\n",
            "stores?tab=1#top",
            "/?tab=1",
            "../x?tab=1",
            "https://evil.example.com/x?a=1",
            &format!("?q={}", "a".repeat(MAX_SCREEN_CHARS)),
        ] {
            assert!(!is_screen_path(bad), "{bad:?}");
        }
        // A colon that follows no scheme is a path's own.
        assert!(is_screen_path("1:30"));
        assert!(is_screen_path("stores/a:b"));
    }

    #[test]
    fn the_declared_list_keeps_what_passes_once_in_order_and_null_is_declares_none() {
        let manifest = json!({ "slug": "store", "checks": { "screens": [
            "", "stores", "/abs", "stores", "reorder/plans", 7, null, "x?y=1", "../up",
            "?module=waste", "x?y=1", "x?", "x#top",
        ] } });
        assert_eq!(
            declared_screens(Some(&manifest)),
            Some(vec![
                "".to_string(),
                "stores".to_string(),
                "reorder/plans".to_string(),
                // A query is the screen's own: kept as written, once.
                "x?y=1".to_string(),
                "?module=waste".to_string(),
            ])
        );
        // Declared, with nothing that passes: an empty list, which the runner
        // reads as the root alone. Never `null`, which means "discover".
        let nothing = json!({ "checks": { "screens": ["/a", "b#c"] } });
        assert_eq!(declared_screens(Some(&nothing)), Some(Vec::new()));
        assert_eq!(
            declared_screens(Some(&json!({ "checks": { "screens": [] } }))),
            Some(Vec::new())
        );
        for none in [
            json!({ "slug": "store" }),
            json!({ "checks": {} }),
            json!({ "checks": { "screens": "stores" } }),
            json!({ "checks": { "screens": null } }),
            json!("nope"),
        ] {
            assert_eq!(declared_screens(Some(&none)), None, "{none}");
        }
        assert_eq!(declared_screens(None), None);
    }

    #[test]
    fn at_most_thirty_screens_are_kept() {
        let many: Vec<String> = (0..45).map(|i| format!("s{i}")).collect();
        let kept = declared_screens(Some(&json!({ "checks": { "screens": many } }))).unwrap();
        assert_eq!(kept.len(), MAX_SCREENS);
        assert_eq!((kept[0].as_str(), kept[29].as_str()), ("s0", "s29"));
    }

    #[test]
    fn the_limits_are_the_contracts() {
        assert_eq!(
            serde_json::to_value(LIMITS).unwrap(),
            json!({ "screens": 30, "file_bytes": 2_000_000, "files": 200 })
        );
    }
}
