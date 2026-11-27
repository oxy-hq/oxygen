//! The promote checks gate (`custom_apps_promote::checks_gate`, the org's
//! `promote_checks_gate` flag), through the real routers in a staff person's
//! session, on the fixture every promote test uses: an app whose production
//! serves `agent-prod` and whose staging serves `agent-stg`, both published by
//! the guest, who is the org's owner and Oxy staff.
//!
//! | Module | Proves |
//! | --- | --- |
//! | `gate` | flag off: the plan has no `checks`, an attempt has four kinds and nothing is held, an override included; flag on: an attempt has six kinds (`changes` advisory), and `checks_running`, `checks_not_passed` and `changes_not_seen` each refuse the promote route and Make Live until the newest attempt satisfies what the app requires; a promoting publish is refused whole |
//! | `moved_on` | a build staging moved past before its runner claimed: the checks that need staging are closed `skipped` and marked `not_checked`, the attempt reads `none`, the promote is `checks_not_passed`, and only the audited override clears it |
//! | `writers` | every writer of production that skips the plan is held: Make Live, the console's rollback to a build production never served, batch publish and promote latest, each `checks_not_passed`; an override clears the checks per app and is audited, and never `staging_never_served`; a rollback to a build production served is never held, by either route |
//! | `overrides` | an org admin's override with a reason moves production, is shown in the plan and writes `app.promote_checks_overridden`; a short reason is `400 override_reason_required`; a caller who is not an officer of the org, and a publish token, are `403 override_not_allowed`; it does not clear `validation_not_passed` |
//! | `settings` | `PUT …/promotion-checks` stores the settings, answers them on the app, audits a change once, refuses `validation`, `changes` and an unknown kind; a publish token cannot set them |

use axum::http::StatusCode;
use chrono::Utc;
use entity::org_members::{self, OrgRole};
use entity::{app_build_checks, feature_flag};
use oxy_api_custom_apps::custom_apps_build_checks::kind::{CheckKind, CheckStatus, Outcome};
use oxy_api_custom_apps::custom_apps_build_checks::store::{self, AttemptKey};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::promote_rollback::person;
use crate::sandbox_agent_token::fixture::Agent;

mod gate;
mod moved_on;
mod overrides;
mod settings;
mod writers;

/// The audit action of an override that cleared a check blocker.
const OVERRIDDEN: &str = "app.promote_checks_overridden";
/// A reason long enough to be one.
const REASON: &str = "outage: shipping the fix before the checks finish";

/// Turn `promote_checks_gate` on for the fixture's org alone, as a staff
/// member's org override does, and load the flag cache from the table.
pub(crate) async fn enable_gate(agent: &Agent) {
    let orgs: serde_json::Map<String, Value> = [(agent.app.org_id.to_string(), json!("on"))]
        .into_iter()
        .collect();
    feature_flag::ActiveModel {
        key: Set(oxy_feature_flags::PROMOTE_CHECKS_GATE.to_string()),
        // The registry default; the kill switch is in the document.
        enabled: Set(false),
        updated_at: Set(Utc::now().fixed_offset()),
        targeting: Set(Some(json!({
            "enabled": true,
            "orgs": orgs,
            "fallthrough": { "type": "variant", "variant": "off" },
        }))),
    }
    .insert(&agent.t.db)
    .await
    .expect("write the gate's org override");
    oxy_feature_flags::cache::load_once(crate::oltp_changes_fixture::flag_db(&agent.t.db))
        .await
        .expect("load the flag cache");
}

/// Every check row of `build`, by attempt then kind.
async fn rows(agent: &Agent, build: Uuid) -> Vec<app_build_checks::Model> {
    let mut rows = app_build_checks::Entity::find()
        .filter(app_build_checks::Column::BuildId.eq(build))
        .all(&agent.t.db)
        .await
        .expect("read the rows");
    rows.sort_by_key(|row| (row.attempt, row.kind.clone()));
    rows
}

/// `(kind, status, advisory)` of one attempt, by kind.
async fn attempt(agent: &Agent, build: Uuid, attempt: i32) -> Vec<(String, String, bool)> {
    rows(agent, build)
        .await
        .into_iter()
        .filter(|row| row.attempt == attempt)
        .map(|row| (row.kind, row.status, row.advisory))
        .collect()
}

/// Give every open check of `attempt` its verdict, as a runner's report and
/// the platform's own writes would: `passed`, but for the kinds in `verdicts`
/// (`changes` is `skipped` unless named).
async fn settle(agent: &Agent, build: Uuid, attempt: i32, verdicts: &[(&str, &str)]) {
    let key = AttemptKey {
        build_pk: build,
        environment: "staging".to_string(),
        attempt,
    };
    for kind in CheckKind::of_staging_attempt(true) {
        let named = verdicts.iter().find(|(name, _)| *name == kind.as_str());
        let status = match (named, kind) {
            (Some((_, status)), _) => status,
            (None, CheckKind::Changes) => "skipped",
            (None, _) => "passed",
        };
        let status = CheckStatus::parse(status).expect("a status");
        let outcome = Outcome::new(status, format!("{} settled", kind.as_str()), &[]);
        store::finish(&agent.t.db, &key, kind, &outcome)
            .await
            .expect("write the verdict");
    }
}

/// Set the `changes` row of `build`'s attempt: `changed` of 28 screens differ,
/// seen by a person or not.
async fn changes(agent: &Agent, build: Uuid, attempt: i32, changed: u64, seen: bool) {
    let row = rows(agent, build)
        .await
        .into_iter()
        .find(|row| row.attempt == attempt && row.kind == "changes")
        .expect("the changes row");
    let seen = seen.then(|| json!({ "by": "ana@oxy.tech", "at": Utc::now().to_rfc3339() }));
    let mut active: app_build_checks::ActiveModel = row.into();
    active.status = Set("passed".to_string());
    active.detail = Set(Some(json!({
        "findings": [], "compared": 28, "changed": changed, "seen": seen,
    })));
    active.update(&agent.t.db).await.expect("write the changes");
}

/// The guest's role in the fixture's org.
async fn set_org_role(agent: &Agent, role: OrgRole) {
    let row = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(agent.app.org_id))
        .filter(org_members::Column::UserId.eq(agent.t.guest_id))
        .one(&agent.t.db)
        .await
        .expect("read the membership")
        .expect("the guest's membership");
    let mut active: org_members::ActiveModel = row.into();
    active.role = Set(role);
    active.update(&agent.t.db).await.expect("set the role");
}

/// The codes of an answer's `blockers`.
fn codes(body: &Value) -> Vec<String> {
    let listed = body["blockers"].as_array().cloned().unwrap_or_default();
    listed
        .iter()
        .filter_map(|blocker| blocker["code"].as_str().map(str::to_string))
        .collect()
}

/// `POST /customer-apps/{id}/publish`: the console's Make Live of the draft.
async fn make_live(agent: &Agent, body: Option<Value>) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/publish", agent.app.id);
    person(agent, "POST", &uri, body).await
}

/// `{"override_checks": {"reason": …}}` merged into `body`.
fn overriding(mut body: Value, reason: &str) -> Value {
    body["override_checks"] = json!({ "reason": reason });
    body
}
