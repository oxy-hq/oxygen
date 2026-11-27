//! What the platform gives the `design_system` check: the SDK floor, and the
//! build's own CSS.
//!
//! The verdict is the runner's. It compares the build's recorded
//! `@oxy-hq/sdk` version (`app_builds.sdk_version`, sent by `oxyc publish`)
//! with [`SDK_FLOOR`], and runs the SDK's theme guard
//! (`checkThemeCss(sources, { built: true })`) over what [`stylesheets`]
//! answers: `index.html` (its `<style>` blocks) and every `.css` under
//! `assets/`, read from the build store.
//!
//! **The size cap.** A bundle's CSS is a few hundred kilobytes. Past
//! [`MAX_STYLESHEET_BYTES`] in total the read is refused whole
//! ([`StylesheetsError::TooLarge`]) rather than cut short: a guard that read
//! half the CSS would pass a build it never judged, so the runner reports
//! the check as `error` with that reason instead.

use entity::app_builds;
use serde::Serialize;

use crate::custom_apps_build_store as store;

/// The lowest `@oxy-hq/sdk` a build may be built with: the release whose
/// theme the design system is built on (one theme, the guard, the desk UI).
pub const SDK_FLOOR: &str = "2.30.0";

/// The most `index.html` and CSS, in bytes, one build's check reads.
pub const MAX_STYLESHEET_BYTES: usize = 8 * 1024 * 1024;

/// Where Vite writes a build's stylesheets.
const ASSETS_DIR: &str = "assets";

/// One file the theme guard reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    /// The path inside the build: `index.html`, `assets/index-abc.css`.
    pub file: String,
    /// The whole file: a stylesheet, or the HTML document.
    pub css: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StylesheetsError {
    #[error("{0}")]
    Store(#[from] store::BuildStoreError),
    #[error(
        "the build's index.html and stylesheets total {bytes} bytes, more than the {max} the \
         design-system check reads"
    )]
    TooLarge { bytes: usize, max: usize },
}

/// `index.html` and every `.css` under `assets/`, in that order.
pub async fn stylesheets(build: &app_builds::Model) -> Result<Vec<Source>, StylesheetsError> {
    let mut sources = Vec::new();
    if let Some(index) = store::get_object(build.app_id, &build.build_id, "index.html").await? {
        sources.push(Source {
            file: "index.html".to_string(),
            css: String::from_utf8_lossy(&index).into_owned(),
        });
    }
    // A bundle with no `assets/` lists nothing.
    let listed = store::files_under(build.app_id, &build.build_id, ASSETS_DIR, ".css").await?;
    for (path, bytes) in listed {
        sources.push(Source {
            file: path,
            css: String::from_utf8_lossy(&bytes).into_owned(),
        });
    }
    within(sources, MAX_STYLESHEET_BYTES)
}

/// `sources` when they total at most `max` bytes.
fn within(sources: Vec<Source>, max: usize) -> Result<Vec<Source>, StylesheetsError> {
    let bytes: usize = sources.iter().map(|source| source.css.len()).sum();
    if bytes > max {
        return Err(StylesheetsError::TooLarge { bytes, max });
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str, bytes: usize) -> Source {
        Source {
            file: file.into(),
            css: "a".repeat(bytes),
        }
    }

    #[test]
    fn sources_past_the_cap_are_refused_whole_and_at_the_cap_are_kept() {
        let at_the_cap = vec![source("index.html", 6), source("assets/a.css", 4)];
        assert_eq!(within(at_the_cap.clone(), 10).unwrap(), at_the_cap);
        let over = vec![source("index.html", 6), source("assets/a.css", 5)];
        let refused = within(over, 10).unwrap_err();
        assert!(
            matches!(refused, StylesheetsError::TooLarge { bytes: 11, max: 10 }),
            "{refused}"
        );
        assert!(refused.to_string().contains("11 bytes"), "{refused}");
        assert!(within(Vec::new(), 0).unwrap().is_empty());
    }

    #[test]
    fn a_source_is_the_wire_shape_the_runner_reads() {
        let wire = serde_json::to_value(source("assets/a.css", 1)).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({ "file": "assets/a.css", "css": "a" })
        );
    }
}
