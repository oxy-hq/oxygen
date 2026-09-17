use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// One location as every reader sees it: the row plus what each integration
/// calls it.
#[derive(Debug, Clone, Serialize)]
pub struct LocationRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub kind: Option<String>,
    pub parent_id: Option<Uuid>,
    pub status: String,
    pub timezone: String,
    /// The tenant's own id, kept from the first version of the table.
    pub external_id: Option<String>,
    /// `system` → id. Sorted, so two reads of the same place serialise alike.
    pub external_ids: BTreeMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Deserialise a field so that *absent*, *null* and *a value* stay three
/// different things — `None`, `Some(None)`, `Some(Some(v))`. A PATCH that
/// says `"parent_id": null` is moving a store to the top level; one that says
/// nothing is leaving it where it is.
pub fn patch<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateLocation {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "patch")]
    pub kind: Option<Option<String>>,
    #[serde(default, deserialize_with = "patch")]
    pub parent_id: Option<Option<Uuid>>,
    pub status: Option<String>,
    pub timezone: Option<String>,
    /// The tenant's own id for the place — what an app keys it by
    /// (`santa-clara`). Not the per-system `external_ids` map. `null` and a
    /// blank string both clear it.
    #[serde(default, deserialize_with = "patch")]
    pub external_id: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SetExternalId {
    pub external_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRole {
    pub name: String,
}

/// A position at a place (or org-wide), as a request names it. Reused by
/// enrolment, where the person does not exist yet when the targets are
/// checked.
#[derive(Debug, Clone, Deserialize)]
pub struct AssignmentSpec {
    pub role_id: Uuid,
    #[serde(default)]
    pub location_id: Option<Uuid>,
    #[serde(default)]
    pub supervisor_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAssignment {
    pub user_id: Uuid,
    #[serde(flatten)]
    pub spec: AssignmentSpec,
}

#[derive(Debug, Default, Deserialize)]
pub struct AssignmentsQuery {
    pub user_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
}

/// One assignment with every name a screen shows, so a roster renders from
/// one read.
#[derive(Debug, Clone, Serialize)]
pub struct AssignmentRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_name: String,
    /// `member` — holds an `org_members` row; `frontline` — a worker.
    pub user_kind: &'static str,
    pub role_id: Uuid,
    pub role_name: String,
    pub role_scope: String,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    pub supervisor_id: Option<Uuid>,
    pub supervisor_name: Option<String>,
    pub created_at: String,
}

/// The slice of an assignment a worker's own row carries.
#[derive(Debug, Clone, Serialize)]
pub struct WorkerAssignment {
    pub id: Uuid,
    pub role_id: Uuid,
    pub role_name: String,
    pub role_scope: String,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    pub supervisor_id: Option<Uuid>,
}

impl From<&AssignmentRow> for WorkerAssignment {
    fn from(a: &AssignmentRow) -> Self {
        Self {
            id: a.id,
            role_id: a.role_id,
            role_name: a.role_name.clone(),
            role_scope: a.role_scope.clone(),
            location_id: a.location_id,
            location_name: a.location_name.clone(),
            supervisor_id: a.supervisor_id,
        }
    }
}

pub const LOCATION_STATUSES: [&str; 5] =
    ["pre_launch", "launching", "open", "archived", "terminated"];

/// Validated here so a typo answers 400 with the word, not the CHECK
/// constraint's 500.
pub fn is_location_status(s: &str) -> bool {
    LOCATION_STATUSES.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_tells_absent_from_null_from_value() {
        let absent: UpdateLocation = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert!(absent.parent_id.is_none() && absent.kind.is_none());
        let cleared: UpdateLocation =
            serde_json::from_str(r#"{"parent_id":null,"kind":null}"#).unwrap();
        assert_eq!(cleared.parent_id, Some(None));
        assert_eq!(cleared.kind, Some(None));
        let set: UpdateLocation = serde_json::from_str(
            r#"{"parent_id":"00000000-0000-0000-0000-000000000001","kind":"store"}"#,
        )
        .unwrap();
        assert!(matches!(set.parent_id, Some(Some(_))));
        assert_eq!(set.kind, Some(Some("store".into())));
    }

    #[test]
    fn an_assignment_request_flattens_its_spec() {
        let req: CreateAssignment = serde_json::from_str(
            r#"{"user_id":"00000000-0000-0000-0000-000000000001","role_id":"00000000-0000-0000-0000-000000000002"}"#,
        )
        .unwrap();
        assert!(req.spec.location_id.is_none() && req.spec.supervisor_id.is_none());
    }

    #[test]
    fn only_the_five_statuses_are_settable() {
        for s in LOCATION_STATUSES {
            assert!(is_location_status(s));
        }
        for s in ["", "Open", "closed", "pre-launch"] {
            assert!(!is_location_status(s), "{s}");
        }
    }
}
