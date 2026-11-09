//! Which pricing refusal, if any, the report may quote as the reason no MDE was
//! priced: only the newest proposal before the run, only when it was refused,
//! and only when it priced the design that was frozen. A refusal a later
//! pricing replaced, or one about an earlier draft, is not the reason.

use entity::experiment_proposals;

use super::proposal_store::ProposalStatus;

/// `newest` is the newest proposal (any status) logged before the run.
pub fn refusal_to_quote(
    newest: Option<&experiment_proposals::Model>,
    frozen_hash: &str,
) -> Option<String> {
    let p = newest?;
    if p.status != ProposalStatus::Refused.as_str() {
        return None;
    }
    if p.design_hash.as_deref() != Some(frozen_hash) {
        return None;
    }
    p.refusal.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn proposal(status: &str, hash: Option<&str>) -> experiment_proposals::Model {
        experiment_proposals::Model {
            id: Uuid::nil(),
            created_at: Utc::now().fixed_offset(),
            workspace_id: Uuid::nil(),
            name: "e".into(),
            design_id: None,
            design_hash: hash.map(str::to_string),
            request_id: Uuid::nil(),
            status: status.into(),
            request: serde_json::json!({}),
            alpha: 0.05,
            seed: None,
            waves: None,
            periods: None,
            strata: None,
            assign_by: None,
            power: None,
            mde_unit_day: None,
            prior_coefficient: None,
            warnings: serde_json::json!([]),
            refusal: (status == "refused").then(|| "too little history".to_string()),
            requested_by: Uuid::nil(),
            finished_at: None,
        }
    }

    #[test]
    fn the_newest_refusal_of_the_frozen_design_is_quoted() {
        let p = proposal("refused", Some("h1"));
        assert_eq!(
            refusal_to_quote(Some(&p), "h1").as_deref(),
            Some("too little history")
        );
    }

    #[test]
    fn a_refusal_a_later_pricing_replaced_is_not_quoted() {
        let newest = proposal("done", Some("h1"));
        assert_eq!(refusal_to_quote(Some(&newest), "h1"), None);
    }

    #[test]
    fn a_refusal_about_an_earlier_draft_is_not_quoted() {
        let p = proposal("refused", Some("h0"));
        assert_eq!(refusal_to_quote(Some(&p), "h1"), None);
        let unhashed = proposal("refused", None);
        assert_eq!(refusal_to_quote(Some(&unhashed), "h1"), None);
    }

    #[test]
    fn no_proposal_quotes_nothing() {
        assert_eq!(refusal_to_quote(None, "h1"), None);
    }
}
