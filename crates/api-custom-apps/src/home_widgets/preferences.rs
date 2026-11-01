//! A person's own home: which widget copies they hid, and whether Today shows.
//!
//! - `GET /api/home/preferences?org=<slug>` — `{ hidden, today, sections }`.
//! - `PUT /api/home/preferences?org=<slug>` — body `{ hidden, today }`; answers
//!   what was stored, with `sections`.
//!
//! `sections` is the org layout's section kinds in the order
//! `GET /api/home/widgets` draws them (`["apps", "insights"]`; the default
//! order with no stored layout). Kinds only: no widget runs and no item is
//! named, so the web app can order the home, and know whether the org kept
//! Insights, while Today is hidden and the widgets are not fetched at all. The
//! viewer and hidden filters do not apply: a group left empty for this person
//! still counts. A body without `sections` is accepted; it is never read.
//!
//! Flat, like `GET /api/home/widgets`, and decided here: the caller must belong
//! to the org (anything else is 404), and the row is theirs alone. The choices
//! only ever take things off their own home; `GET /api/home/widgets` applies them.
//! `internal-docs/home-widgets.md`.

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::Query;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use oxy_app_core::widgets::layout::{Drawn, Layout, read, resolve};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseConnection, DbErr};
use serde_json::{Value, json};
use tracing::instrument;
use uuid::Uuid;

use super::viewer::asker;
use super::{ListQuery, internal, layout_store, preferences_store};

/// The most copies a person may hide.
pub const MAX_HIDDEN: usize = 100;
/// The longest copy key (`<app>/<widget>#<id>`) that is stored.
pub const MAX_KEY_CHARS: usize = 200;

/// A person's choices in one org.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    /// Copy keys, in the order they were hidden, each once.
    pub hidden: Vec<String>,
    pub today_hidden: bool,
}

impl Preferences {
    /// `"shown"` or `"hidden"`: how the API names the Today choice.
    pub fn today(&self) -> &'static str {
        if self.today_hidden { "hidden" } else { "shown" }
    }

    pub fn to_json(&self) -> Value {
        json!({ "hidden": self.hidden, "today": self.today() })
    }

    /// The answer of both routes: the choices, and the org layout's section kinds.
    pub fn answer(&self, sections: &[&str]) -> Value {
        json!({ "hidden": self.hidden, "today": self.today(), "sections": sections })
    }
}

/// The kinds of the sections `layout` draws, in order: `"widgets"`, `"apps"`,
/// `"insights"`. `None` is the default home. Resolved against no catalog, which
/// changes what a group holds and never which sections there are.
pub fn section_kinds(layout: Option<&Layout>) -> Vec<&'static str> {
    resolve(layout, &[])
        .iter()
        .map(|d| match d {
            Drawn::Widgets { .. } => "widgets",
            Drawn::Apps => "apps",
            Drawn::Insights => "insights",
        })
        .collect()
}

/// The section kinds of `org_id`'s stored layout; a stored one that no longer
/// parses reads as none, as it does for `GET /api/home/widgets`.
pub async fn org_section_kinds(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<&'static str>, DbErr> {
    let layout = layout_store::load(db, org_id)
        .await?
        .and_then(|stored| read(&stored.layout));
    Ok(section_kinds(layout.as_ref()))
}

/// A request body, checked: the choices, or why not.
pub fn parse(body: &Value) -> Result<Preferences, String> {
    let Some(obj) = body.as_object() else {
        return Err("send { \"hidden\": [ … ], \"today\": \"shown\" | \"hidden\" }".into());
    };
    let today_hidden = match obj.get("today").and_then(Value::as_str) {
        Some("shown") => false,
        Some("hidden") => true,
        _ => return Err("`today` is \"shown\" or \"hidden\"".into()),
    };
    let Some(list) = obj.get("hidden").and_then(Value::as_array) else {
        return Err("`hidden` is a list of widget keys".into());
    };
    let mut seen = BTreeSet::new();
    let mut hidden = Vec::new();
    for key in list {
        let Some(key) = key
            .as_str()
            .filter(|k| (1..=MAX_KEY_CHARS).contains(&k.chars().count()))
        else {
            return Err(format!(
                "each entry of `hidden` is a widget key of 1 to {MAX_KEY_CHARS} characters"
            ));
        };
        if seen.insert(key) {
            hidden.push(key.to_string());
        }
    }
    if hidden.len() > MAX_HIDDEN {
        return Err(format!("`hidden` lists at most {MAX_HIDDEN} widgets"));
    }
    Ok(Preferences {
        hidden,
        today_hidden,
    })
}

/// `GET /api/home/preferences?org=<slug>`
#[instrument(skip_all, fields(org = %q.org))]
pub async fn get_preferences(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<ListQuery>,
) -> Response {
    let who = match asker(&user, &q.org).await {
        Ok(who) => who,
        Err(status) => return status.into_response(),
    };
    let Some(org_id) = who.org_id else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let loaded = async {
        let prefs = preferences_store::load(&who.db, user.id, org_id).await?;
        let sections = org_section_kinds(&who.db, org_id).await?;
        Ok::<_, DbErr>(prefs.answer(&sections))
    };
    match loaded.await {
        Ok(answer) => Json(answer).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// `PUT /api/home/preferences?org=<slug>`
#[instrument(skip_all, fields(org = %q.org))]
pub async fn put_preferences(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    // A write: production only, decided from the headers before anything is read.
    if let Err(refused) =
        oxy_app_core::custom_app_env_request::guard(&Method::PUT, "/api/home/preferences", &headers)
    {
        return *refused;
    }
    let who = match asker(&user, &q.org).await {
        Ok(who) => who,
        Err(status) => return status.into_response(),
    };
    let Some(org_id) = who.org_id else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let prefs = match parse(&body) {
        Ok(prefs) => prefs,
        Err(message) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "preferences_invalid", "message": message })),
            )
                .into_response();
        }
    };
    let saved = async {
        preferences_store::save(&who.db, user.id, org_id, &prefs).await?;
        let sections = org_section_kinds(&who.db, org_id).await?;
        Ok::<_, DbErr>(prefs.answer(&sections))
    };
    match saved.await {
        Ok(answer) => Json(answer).into_response(),
        Err(e) => internal(e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_body_is_read_and_deduped_in_order() {
        let prefs =
            parse(&json!({ "hidden": ["b/x", "a/y#z", "b/x"], "today": "hidden" })).unwrap();
        assert_eq!(prefs.hidden, ["b/x", "a/y#z"]);
        assert!(prefs.today_hidden);
        assert_eq!(
            prefs.to_json(),
            json!({ "hidden": ["b/x", "a/y#z"], "today": "hidden" })
        );
    }

    #[test]
    fn the_answer_carries_the_layouts_section_kinds_in_order() {
        let prefs = Preferences::default();
        // No stored layout: the default home.
        assert_eq!(
            prefs.answer(&section_kinds(None)),
            json!({ "hidden": [], "today": "shown", "sections": ["widgets", "apps", "insights"] })
        );
        // A layout that dropped Insights and put Apps first.
        let layout = read(&json!({ "version": 1, "sections": [
            { "kind": "apps" },
            { "kind": "widgets", "title": "Back office", "items": [] }
        ] }))
        .expect("a valid layout");
        assert_eq!(section_kinds(Some(&layout)), ["apps", "widgets"]);
        let hidden = Preferences {
            hidden: vec!["a/w".into()],
            today_hidden: true,
        };
        assert_eq!(
            hidden.answer(&section_kinds(Some(&layout))),
            json!({ "hidden": ["a/w"], "today": "hidden", "sections": ["apps", "widgets"] })
        );
    }

    #[test]
    fn a_body_without_sections_is_accepted() {
        // The web app's optimistic PUT sends `{ hidden, today }` only.
        assert!(parse(&json!({ "hidden": [], "today": "hidden" })).is_ok());
        assert!(parse(&json!({ "hidden": [], "today": "hidden", "sections": ["apps"] })).is_ok());
    }

    #[test]
    fn the_default_is_nothing_hidden_and_today_shown() {
        assert_eq!(
            Preferences::default().to_json(),
            json!({ "hidden": [], "today": "shown" })
        );
    }

    #[test]
    fn the_limits_are_enforced() {
        let many: Vec<String> = (0..=MAX_HIDDEN).map(|i| format!("a/w{i}")).collect();
        assert!(parse(&json!({ "hidden": many, "today": "shown" })).is_err());
        let exactly: Vec<String> = (0..MAX_HIDDEN).map(|i| format!("a/w{i}")).collect();
        assert!(parse(&json!({ "hidden": exactly, "today": "shown" })).is_ok());
        let long = "k".repeat(MAX_KEY_CHARS + 1);
        assert!(parse(&json!({ "hidden": [long], "today": "shown" })).is_err());
        let fits = "k".repeat(MAX_KEY_CHARS);
        assert!(parse(&json!({ "hidden": [fits], "today": "shown" })).is_ok());
        // Duplicates do not count against the cap.
        let dupes = vec!["a/w"; MAX_HIDDEN + 5];
        assert!(parse(&json!({ "hidden": dupes, "today": "shown" })).is_ok());
    }

    #[test]
    fn a_malformed_body_is_refused() {
        for bad in [
            json!([]),
            json!({ "hidden": [] }),
            json!({ "today": "shown" }),
            json!({ "hidden": [], "today": "maybe" }),
            json!({ "hidden": [], "today": true }),
            json!({ "hidden": "a/b", "today": "shown" }),
            json!({ "hidden": [""], "today": "shown" }),
            json!({ "hidden": [7], "today": "shown" }),
        ] {
            assert!(parse(&bad).is_err(), "{bad} should be refused");
        }
    }
}
