//! The promote checks gate's two switches: whether it is on for an org (the
//! `promote_checks_gate` flag), and what one app asks of it
//! (`apps.promotion_checks`).
//!
//! **The flag decides everything new.** Off, a staging attempt has phase 1's
//! four kinds, no check holds a move, and an app's settings are stored and
//! read by nothing. On, an attempt also has `smoke` and `changes`, and the
//! kinds the app requires hold its promote to production
//! (`custom_apps_promote::checks_gate`).
//!
//! **An app's settings** are one nullable JSONB object, the platform's own
//! configuration of the app like `require_second_person_to_promote`:
//!
//! | Field | Null column | Means |
//! | --- | --- | --- |
//! | `required` | `null` | the kinds a promote waits for; `null` is every kind that can be required ([`REQUIRABLE`]), `[]` none |
//! | `seen_required` | `false` | screens that differ from production need a person's look before a promote |
//! | `production_smoke` | `true` | open the same screens on production once it serves the build |
//! | `auto_rollback` | `false` | roll production back when that smoke fails |
//!
//! `validation` is never listed: a build whose validation did not pass is
//! refused by its own blocker (`validation_not_passed`), which no setting and
//! no override clears. `changes` is advisory and cannot be required either;
//! `seen_required` is how it holds a promote.

use entity::apps;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::kind::CheckKind;

/// Whether the promote checks gate is on for `org_id`.
pub fn enabled_for(org_id: Uuid) -> bool {
    use oxy_feature_flags::{FlagContext, PROMOTE_CHECKS_GATE, is_enabled_for};
    is_enabled_for(PROMOTE_CHECKS_GATE, &FlagContext::org(org_id))
}

/// The staging kinds an app may require, in a build's list order: what it
/// requires when it chose none.
pub const REQUIRABLE: [CheckKind; 4] = [
    CheckKind::Functions,
    CheckKind::Loads,
    CheckKind::DesignSystem,
    CheckKind::Smoke,
];

fn yes() -> bool {
    true
}

/// One app's settings, as stored and as `PUT …/promotion-checks` sends them.
/// A field the request leaves out takes its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The kinds the app chose; `None` is the defaults.
    #[serde(default)]
    pub required: Option<Vec<String>>,
    #[serde(default)]
    pub seen_required: bool,
    #[serde(default = "yes")]
    pub production_smoke: bool,
    #[serde(default)]
    pub auto_rollback: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            required: None,
            seen_required: false,
            production_smoke: true,
            auto_rollback: false,
        }
    }
}

/// What an app's response carries (`AppResponse.promotion_checks`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SettingsDto {
    /// The org's `promote_checks_gate` flag.
    pub enabled: bool,
    pub required: Option<Vec<String>>,
    /// What `required` comes to: itself, or the defaults when it is null.
    pub effective_required: Vec<String>,
    pub seen_required: bool,
    pub production_smoke: bool,
    pub auto_rollback: bool,
}

impl Settings {
    /// The settings `app` holds; the defaults for a null column, and for one
    /// this build cannot read (logged: nothing an admin chose is dropped
    /// silently).
    pub fn of(app: &apps::Model) -> Self {
        let Some(stored) = &app.promotion_checks else {
            return Self::default();
        };
        serde_json::from_value(stored.clone()).unwrap_or_else(|e| {
            tracing::warn!(app_id = %app.id, error = %e,
                "apps.promotion_checks is unreadable; the defaults apply");
            Self::default()
        })
    }

    /// The kinds a promote waits for, in a build's list order. A stored name
    /// that is not a kind an app can require is no requirement.
    pub fn effective_required(&self) -> Vec<CheckKind> {
        match &self.required {
            None => REQUIRABLE.to_vec(),
            Some(chosen) => REQUIRABLE
                .into_iter()
                .filter(|kind| chosen.iter().any(|name| name == kind.as_str()))
                .collect(),
        }
    }

    /// The settings a request asks for, as they are stored: `required` held
    /// to the kinds that can be required, without repeats, in list order.
    /// `Err` is the sentence a `400` answers with.
    pub fn validated(self) -> Result<Self, String> {
        let Some(chosen) = &self.required else {
            return Ok(self);
        };
        for name in chosen {
            if !REQUIRABLE.iter().any(|kind| kind.as_str() == name) {
                return Err(not_requirable(name));
            }
        }
        let required = self.effective_required();
        Ok(Self {
            required: Some(required.into_iter().map(names).collect()),
            ..self
        })
    }

    /// The settings as an app's response shows them.
    pub fn dto(&self, enabled: bool) -> SettingsDto {
        SettingsDto {
            enabled,
            required: self.required.clone(),
            effective_required: self.effective_required().into_iter().map(names).collect(),
            seen_required: self.seen_required,
            production_smoke: self.production_smoke,
            auto_rollback: self.auto_rollback,
        }
    }
}

fn names(kind: CheckKind) -> String {
    kind.as_str().to_string()
}

/// Why `name` cannot be listed in `required`.
fn not_requirable(name: &str) -> String {
    let can: Vec<&str> = REQUIRABLE.iter().map(|kind| kind.as_str()).collect();
    let can = can.join(", ");
    match name {
        "validation" => format!(
            "`validation` cannot be listed in `required`: a build whose validation did not pass \
             is always refused (`validation_not_passed`), whatever an app requires. List any of: \
             {can}"
        ),
        "changes" => format!(
            "`changes` cannot be listed in `required`: it only informs. To hold a promote until \
             a person has looked at what changed, set `seen_required`. List any of: {can}"
        ),
        other => format!(
            "`{}` is not a check an app can require. List any of: {can}",
            super::kind::cap(other, 40)
        ),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn app(stored: Option<serde_json::Value>) -> apps::Model {
        apps::Model {
            id: Uuid::from_u128(1),
            slug: "store".into(),
            name: "Store".into(),
            org_id: Uuid::from_u128(2),
            project_id: Uuid::from_u128(3),
            branch: "main".into(),
            source_repo: String::new(),
            status: "active".into(),
            source_type: "s3".into(),
            source_config: json!({}),
            last_synced_at: None,
            manifest_override: None,
            bootstrap_pr_url: None,
            published_at: None,
            repo_path: None,
            last_promoted_by: None,
            last_promoted_at: None,
            visibility: "org".into(),
            require_second_person_to_promote: false,
            promotion_checks: stored,
            created_at: Default::default(),
            updated_at: Default::default(),
        }
    }

    const DEFAULTS: [&str; 4] = ["functions", "loads", "design_system", "smoke"];

    /// A null column, and a request that names nothing, are the defaults, and
    /// the response shows them as the contract writes them.
    #[test]
    fn a_null_column_is_the_defaults() {
        let settings = Settings::of(&app(None));
        assert_eq!(settings, Settings::default());
        assert_eq!(
            serde_json::from_value::<Settings>(json!({})).unwrap(),
            settings
        );
        assert_eq!(
            serde_json::to_value(settings.dto(true)).unwrap(),
            json!({
                "enabled": true,
                "required": null,
                "effective_required": DEFAULTS,
                "seen_required": false,
                "production_smoke": true,
                "auto_rollback": false,
            })
        );
        assert!(!settings.dto(false).enabled);
        // Stored garbage reads as the defaults, never as "nothing required".
        let garbled = Settings::of(&app(Some(json!("yes"))));
        assert_eq!(garbled, Settings::default());
    }

    #[test]
    fn a_chosen_list_is_kept_in_list_order_without_repeats() {
        let asked: Settings = serde_json::from_value(json!({
            "required": ["smoke", "functions", "smoke"],
            "seen_required": true,
            "production_smoke": false,
            "auto_rollback": true,
        }))
        .unwrap();
        let stored = asked.validated().unwrap();
        assert_eq!(
            stored.required,
            Some(vec!["functions".to_string(), "smoke".to_string()])
        );
        assert_eq!(
            stored.effective_required(),
            [CheckKind::Functions, CheckKind::Smoke]
        );
        assert!(stored.seen_required && !stored.production_smoke && stored.auto_rollback);
        // What is stored reads back as itself.
        let back = Settings::of(&app(Some(serde_json::to_value(&stored).unwrap())));
        assert_eq!(back, stored);

        // An empty list requires nothing, and is not the defaults.
        let none = Settings {
            required: Some(Vec::new()),
            ..Settings::default()
        };
        let none = none.validated().unwrap();
        assert_eq!(none.required, Some(Vec::new()));
        assert!(none.effective_required().is_empty());
        assert!(none.dto(true).effective_required.is_empty());
    }

    #[test]
    fn validation_changes_and_an_unknown_kind_are_refused_saying_why() {
        let asking = |kind: &str| Settings {
            required: Some(vec!["loads".to_string(), kind.to_string()]),
            ..Settings::default()
        };
        let validation = asking("validation").validated().unwrap_err();
        assert!(validation.contains("validation_not_passed"), "{validation}");
        let changes = asking("changes").validated().unwrap_err();
        assert!(changes.contains("seen_required"), "{changes}");
        for unknown in ["production_smoke", "design_audit", "summary", "Loads", ""] {
            let refused = asking(unknown).validated().unwrap_err();
            assert!(
                refused.contains("functions, loads, design_system, smoke"),
                "{unknown:?}: {refused}"
            );
        }
        // A stored name this build does not know is no requirement.
        let newer = Settings::of(&app(Some(json!({ "required": ["loads", "later_kind"] }))));
        assert_eq!(newer.effective_required(), [CheckKind::Loads]);
    }
}
