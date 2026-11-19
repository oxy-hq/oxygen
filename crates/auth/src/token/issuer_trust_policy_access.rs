//! The request bodies of the OIDC issuer trust-policy routes, parsed and
//! checked (API-tokens design §3.4, *Any OIDC issuer*). Pure.
//!
//! What a body may ask for:
//!
//! - `issuer_id` is a registered issuer of the account's own org — checked
//!   against the database by the handler, not here.
//! - `conditions` are claim conditions as
//!   [`crate::workload_identity::conditions::check`] takes them: top-level
//!   scalar claims, exact values or a prefix with one trailing `*`, at least
//!   one not on `aud`, never `iss`.
//! - `grants` is **always explicit**, with the GitHub policies' rules
//!   ([`super::trust_policy_access::policy_grants`]), and **the whole
//!   organization or one app, never one workspace**: workspaces are being
//!   retired, so this new kind of policy does not add a way to name one.
//!
//! Which issuer and conditions a policy names never changes: a policy for
//! another token is another policy. An edit changes its grants, or disables
//! it.

use serde::Deserialize;
use uuid::Uuid;

use super::access::{GrantInput, Invalid, invalid};
use super::account_access::AccountRole;
use super::trust_policy_access::{PolicyGrant, policy_grants};
use crate::workload_identity::conditions::{self, Condition};

/// `POST …/oidc-issuer-trust-policies`.
#[derive(Clone, Debug, Deserialize)]
pub struct CreateIssuerPolicyBody {
    pub issuer_id: Uuid,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    pub grants: Option<Vec<GrantInput>>,
}

/// A create, checked for shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuerPolicyWant {
    pub issuer_id: Uuid,
    /// Checked, trimmed, and ordered by claim.
    pub conditions: Vec<Condition>,
    pub grants: Vec<PolicyGrant>,
}

impl CreateIssuerPolicyBody {
    pub fn want(&self, org_id: Uuid, role: AccountRole) -> Result<IssuerPolicyWant, Invalid> {
        Ok(IssuerPolicyWant {
            issuer_id: self.issuer_id,
            conditions: conditions::check(&self.conditions).map_err(Invalid)?,
            grants: issuer_policy_grants(org_id, role, self.grants.as_deref().unwrap_or(&[]))?,
        })
    }
}

/// [`policy_grants`], refusing a grant that names one workspace.
fn issuer_policy_grants(
    org_id: Uuid,
    role: AccountRole,
    inputs: &[GrantInput],
) -> Result<Vec<PolicyGrant>, Invalid> {
    let grants = policy_grants(org_id, role, inputs)?;
    let names_a_workspace = grants
        .iter()
        .any(|g| matches!(g, PolicyGrant::Workspace(spec) if spec.workspace_id.is_some()));
    if names_a_workspace {
        return invalid(
            "an OIDC issuer trust policy grants the whole organization ('workspace_id': null) or \
             publishing an app, never one workspace",
        );
    }
    Ok(grants)
}

/// `PATCH …/oidc-issuer-trust-policies/{id}`. A field left out is left as it
/// is.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PatchIssuerPolicyBody {
    pub grants: Option<Vec<GrantInput>>,
    pub disabled: Option<bool>,
    /// Never editable: a policy for another issuer or other claims is another
    /// policy.
    pub issuer_id: Option<serde_json::Value>,
    pub conditions: Option<serde_json::Value>,
}

/// An edit, checked for shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IssuerPolicyEdit {
    /// `Some` replaces the whole set.
    pub grants: Option<Vec<PolicyGrant>>,
    pub disabled: Option<bool>,
}

impl PatchIssuerPolicyBody {
    pub fn edit(&self, org_id: Uuid, role: AccountRole) -> Result<IssuerPolicyEdit, Invalid> {
        if self.issuer_id.is_some() || self.conditions.is_some() {
            return invalid(
                "a trust policy's issuer and conditions cannot be changed: create another policy",
            );
        }
        Ok(IssuerPolicyEdit {
            grants: self
                .grants
                .as_deref()
                .map(|inputs| issuer_policy_grants(org_id, role, inputs))
                .transpose()?,
            disabled: self.disabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUER: &str = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    const WHOLE_ORG: &str = r#"[{"kind":"workspace","workspace_id":null,"role_ceiling":"member"}]"#;

    fn body(conditions: &str, grants: &str) -> CreateIssuerPolicyBody {
        serde_json::from_str(&format!(
            r#"{{"issuer_id":"{ISSUER}","conditions":{conditions},"grants":{grants}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn conditions_are_checked_and_kept_ordered() {
        let org = Uuid::from_u128(1);
        let good = body(
            r#"[{"claim":"sub","value":"system:serviceaccount:kestra:worker"},
                {"claim":"aud","value":"oxy:app.oxygen-hq.com"}]"#,
            WHOLE_ORG,
        )
        .want(org, AccountRole::Member)
        .expect("a policy");
        assert_eq!(good.conditions[0].claim, "aud");
        assert_eq!(good.conditions[1].claim, "sub");
        for bad in [
            r#"[]"#,
            r#"[{"claim":"aud","value":"oxy:app.oxygen-hq.com"}]"#,
            r#"[{"claim":"sub","value":"*"}]"#,
            r#"[{"claim":"sub","value":"a*b"}]"#,
            r#"[{"claim":"iss","value":"https://x"},{"claim":"sub","value":"y"}]"#,
        ] {
            assert!(
                body(bad, WHOLE_ORG).want(org, AccountRole::Member).is_err(),
                "{bad}"
            );
        }
        let missing: CreateIssuerPolicyBody = serde_json::from_str(&format!(
            r#"{{"issuer_id":"{ISSUER}","grants":{WHOLE_ORG}}}"#
        ))
        .unwrap();
        assert!(
            missing.want(org, AccountRole::Member).is_err(),
            "no conditions"
        );
    }

    #[test]
    fn what_a_policy_names_cannot_be_edited() {
        let role = AccountRole::Member;
        let org = Uuid::from_u128(1);
        for body in [
            r#"{"conditions":[{"claim":"sub","value":"other"}]}"#,
            r#"{"issuer_id":"3f2504e0-4f89-41d3-9a0c-0305e82c3302"}"#,
        ] {
            let patch: PatchIssuerPolicyBody = serde_json::from_str(body).unwrap();
            assert!(patch.edit(org, role).is_err(), "{body}");
        }
        let patch: PatchIssuerPolicyBody = serde_json::from_str(r#"{"disabled":true}"#).unwrap();
        assert_eq!(
            patch.edit(org, role).ok().and_then(|e| e.disabled),
            Some(true)
        );
    }

    #[test]
    fn grants_are_the_whole_org_or_an_app_never_one_workspace() {
        let org = Uuid::from_u128(1);
        let sub = r#"[{"claim":"sub","value":"x"}]"#;
        assert!(body(sub, WHOLE_ORG).want(org, AccountRole::Member).is_ok());
        let one = r#"[{"kind":"workspace","workspace_id":"3f2504e0-4f89-41d3-9a0c-0305e82c3302",
                       "role_ceiling":"member"}]"#;
        assert!(body(sub, one).want(org, AccountRole::Member).is_err());
        let patch: PatchIssuerPolicyBody =
            serde_json::from_str(&format!(r#"{{"grants":{one}}}"#)).unwrap();
        assert!(patch.edit(org, AccountRole::Member).is_err());
    }

    #[test]
    fn a_policy_states_its_grants() {
        let body: CreateIssuerPolicyBody = serde_json::from_str(&format!(
            r#"{{"issuer_id":"{ISSUER}","conditions":[{{"claim":"sub","value":"x"}}]}}"#
        ))
        .unwrap();
        assert!(body.want(Uuid::from_u128(1), AccountRole::Member).is_err());
    }
}
