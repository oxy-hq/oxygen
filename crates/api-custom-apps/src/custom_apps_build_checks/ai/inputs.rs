//! What the notes are written from, read back from an attempt: the changed
//! screens the `changes` row lists, and the files the runner kept for them
//! (`custom_apps_build_checks::files`): a screen's picture on staging and on
//! production, its visible text on each, and the build's CSS digest.
//!
//! Everything read here was written by a runner about an app's pages, so it
//! is untrusted: a name is used only when it is a kept file's name, a picture
//! only when its bytes start like a PNG, and text is cut to the runner's own
//! caps before it goes anywhere.

use entity::app_build_checks;
use oxy_custom_apps::build_store::check_files::{self, Attempt};
use serde_json::Value;

use super::super::files::FileType;
use super::super::kind::cap;

/// The most of one screen's visible text the model is given: the runner's
/// own cap on a `text-…json` file.
pub const MAX_TEXT_CHARS: usize = 4_000;
/// The most of the CSS digest the model is given: the runner's own cap.
pub const MAX_DIGEST_CHARS: usize = 64 * 1024;
/// The name the runner keeps the CSS digest under.
pub const CSS_DIGEST: &str = "css-digest.json";

/// One changed screen at one viewport, as the `changes` row lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChangedScreen {
    /// The app-relative path; `""` is the root.
    pub path: String,
    pub viewport: String,
    /// The share of pixels that differ, when the runner said.
    pub ratio: Option<f64>,
    /// The kept picture of the screen on staging, by name.
    pub staging: Option<String>,
    /// The same on production.
    pub production: Option<String>,
}

/// A stored name, when it may be a kept file's.
fn file_name(raw: &Value) -> Option<String> {
    raw.as_str()
        .filter(|name| check_files::is_valid_name(name))
        .map(str::to_string)
}

/// The screens a `changes` detail marks `changed`, in its order.
pub fn changed_screens(detail: &Value) -> Vec<ChangedScreen> {
    let screens = detail["screens"].as_array().map(Vec::as_slice);
    screens
        .unwrap_or_default()
        .iter()
        .filter(|screen| screen["changed"].as_bool() == Some(true))
        .map(|screen| ChangedScreen {
            path: cap(screen["path"].as_str().unwrap_or_default(), 256),
            viewport: cap(screen["viewport"].as_str().unwrap_or_default(), 40),
            ratio: screen["ratio"].as_f64().filter(|r| r.is_finite()),
            staging: file_name(&screen["staging"]),
            production: file_name(&screen["production"]),
        })
        .collect()
}

impl ChangedScreen {
    /// The screen as a sentence names it: `screen "stores" at desk`.
    pub fn named(&self) -> String {
        let path = if self.path.is_empty() {
            "the root screen".to_string()
        } else {
            format!("screen {:?}", self.path)
        };
        format!("{path} at {}", self.viewport)
    }

    /// The share of differing pixels, as a sentence says it.
    pub fn differs(&self) -> String {
        match self.ratio {
            Some(ratio) => format!("{:.1}% of its pixels differ", ratio * 100.0),
            None => "its size differs".to_string(),
        }
    }

    /// The name of the screen's kept text in `environment`: its picture's
    /// name with `shot-` and `.png` swapped for `text-` and `.json` (the two
    /// share the runner's slug), else the slug the contract gives the path.
    pub fn text_name(&self, environment: &str) -> String {
        let shot = match environment {
            "staging" => self.staging.as_deref(),
            _ => self.production.as_deref(),
        };
        let from_shot = shot
            .and_then(|name| name.strip_prefix("shot-"))
            .and_then(|name| name.strip_suffix(".png"))
            .map(|stem| format!("text-{stem}.json"));
        from_shot.unwrap_or_else(|| {
            format!(
                "text-{environment}-{}-{}.json",
                slug_of(&self.path),
                self.viewport
            )
        })
    }
}

/// The contract's slug of a path: `/` as `--`, the root as `root`, and any
/// character a file name cannot hold as `-`.
fn slug_of(path: &str) -> String {
    if path.is_empty() {
        return "root".to_string();
    }
    path.to_ascii_lowercase()
        .replace('/', "--")
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '-',
        })
        .collect()
}

/// The files one attempt kept.
#[derive(Clone, Copy, Debug)]
pub struct Files<'a> {
    at: Attempt<'a>,
}

impl<'a> Files<'a> {
    /// The files of `row`'s attempt (any row of it).
    pub fn of(row: &'a app_build_checks::Model) -> Self {
        Self {
            at: Attempt {
                app_id: row.app_id,
                build_pk: row.build_id,
                environment: &row.environment,
                attempt: row.attempt,
            },
        }
    }

    /// The kept file `name`; `None` when there is none or it cannot be read
    /// (logged by name, never by content).
    async fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        match check_files::get(&self.at, name).await {
            Ok(found) => found.map(|bytes| bytes.to_vec()),
            Err(e) => {
                tracing::warn!(app_id = %self.at.app_id, build_pk = %self.at.build_pk, name,
                    error = %e, "build checks notes: a kept file could not be read");
                None
            }
        }
    }

    /// The kept picture `name`: its bytes when they start like a PNG.
    pub async fn picture(&self, name: &str) -> Option<Vec<u8>> {
        let bytes = self.bytes(name).await?;
        (FileType::of(&bytes) == FileType::Png).then_some(bytes)
    }

    /// The visible text a `text-…json` file holds (`{"text": "…"}`), cut to
    /// [`MAX_TEXT_CHARS`]; `None` when there is no such file or it holds none.
    pub async fn text(&self, name: &str) -> Option<String> {
        let bytes = self.bytes(name).await?;
        let stored: Value = serde_json::from_slice(&bytes).ok()?;
        let text = stored["text"].as_str()?.trim();
        (!text.is_empty()).then(|| cap(text, MAX_TEXT_CHARS))
    }

    /// The build's CSS digest, as the lines the model reads; `None` when the
    /// attempt kept none or it lists no declaration.
    pub async fn css_digest(&self) -> Option<String> {
        let bytes = self.bytes(CSS_DIGEST).await?;
        let stored: Value = serde_json::from_slice(&bytes).ok()?;
        digest_lines(&stored)
    }
}

/// A CSS digest (`{"rules": [{file, selector, within?, declarations}],
/// "omitted": n}`) as one line per rule, cut to [`MAX_DIGEST_CHARS`]. `None`
/// when it lists no declaration.
pub fn digest_lines(digest: &Value) -> Option<String> {
    let rules = digest["rules"].as_array()?;
    let mut lines = Vec::new();
    for rule in rules {
        let declarations: Vec<&str> = rule["declarations"]
            .as_array()
            .map(|d| d.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if declarations.is_empty() {
            continue;
        }
        let within: Vec<&str> = rule["within"]
            .as_array()
            .map(|w| w.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let inside = if within.is_empty() {
            String::new()
        } else {
            format!(" (inside {})", within.join(" "))
        };
        lines.push(format!(
            "{} | {}{inside} | {}",
            rule["file"].as_str().unwrap_or("?"),
            rule["selector"].as_str().unwrap_or("?"),
            declarations.join("; ")
        ));
    }
    if lines.is_empty() {
        return None;
    }
    if let Some(omitted) = digest["omitted"].as_u64().filter(|n| *n > 0) {
        lines.push(format!(
            "({omitted} more declarations were left out of the digest)"
        ));
    }
    Some(cap(&lines.join("\n"), MAX_DIGEST_CHARS))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_the_changed_screens_are_read_and_only_names_a_file_can_have() {
        let detail = json!({ "compared": 3, "changed": 2, "screens": [
            { "path": "stores", "viewport": "desk", "changed": true, "ratio": 0.031,
              "staging": "shot-staging-stores-desk.png",
              "production": "shot-production-stores-desk.png" },
            { "path": "", "viewport": "phone", "changed": false, "ratio": 0,
              "staging": "shot-staging-root-phone.png",
              "production": "shot-production-root-phone.png" },
            { "path": "a/b", "viewport": "phone", "changed": true, "ratio": null,
              "staging": "../../etc/passwd", "production": 7 },
        ] });
        let screens = changed_screens(&detail);
        assert_eq!(screens.len(), 2);
        assert_eq!(screens[0].named(), "screen \"stores\" at desk");
        assert_eq!(screens[0].differs(), "3.1% of its pixels differ");
        assert_eq!(
            screens[0].text_name("staging"),
            "text-staging-stores-desk.json"
        );
        assert_eq!(
            screens[0].text_name("production"),
            "text-production-stores-desk.json"
        );
        // A name that is not a kept file's is no name; the text's is then the
        // contract's slug of the path.
        assert_eq!(
            (&screens[1].staging, &screens[1].production),
            (&None, &None)
        );
        assert_eq!(screens[1].differs(), "its size differs");
        assert_eq!(
            screens[1].text_name("staging"),
            "text-staging-a--b-phone.json"
        );
        assert!(changed_screens(&json!({ "screens": "none" })).is_empty());
        assert!(changed_screens(&json!(null)).is_empty());
        let root = ChangedScreen {
            path: String::new(),
            viewport: "desk".into(),
            ratio: Some(0.5),
            staging: None,
            production: None,
        };
        assert_eq!(root.named(), "the root screen at desk");
        assert_eq!(
            root.text_name("production"),
            "text-production-root-desk.json"
        );
    }

    #[test]
    fn a_digest_reads_as_one_line_a_rule_and_an_empty_one_as_none() {
        let digest = json!({ "rules": [
            { "file": "assets/app.css", "selector": ".total", "declarations": ["color: #1a1a1a", "font-size: 11px"] },
            { "file": "assets/app.css", "selector": ".bar", "within": ["@media (min-width: 40rem)"],
              "declarations": ["background: rgb(0 0 0)"] },
            { "file": "index.html", "selector": ".empty", "declarations": [] },
        ], "omitted": 3 });
        assert_eq!(
            digest_lines(&digest).unwrap(),
            "assets/app.css | .total | color: #1a1a1a; font-size: 11px\n\
             assets/app.css | .bar (inside @media (min-width: 40rem)) | background: rgb(0 0 0)\n\
             (3 more declarations were left out of the digest)"
        );
        assert_eq!(digest_lines(&json!({ "rules": [], "omitted": 0 })), None);
        assert_eq!(digest_lines(&json!({ "text": "not a digest" })), None);
        let long = json!({ "rules": [{ "file": "a.css", "selector": ".x",
            "declarations": ["color: #000".repeat(20_000)] }] });
        assert_eq!(
            digest_lines(&long).unwrap().chars().count(),
            MAX_DIGEST_CHARS
        );
    }
}
