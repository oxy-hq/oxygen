//! The parameters a publish statement takes (`events_op`, `apps_send`): the
//! row's fields as this run writes them, and the host's MACs over them
//! (`app_events::provenance`): the row's own, and for a send the invoker's
//! (message domain v2).

use oxy_custom_apps::events::{event_id, qualified_type};
use serde_json::{Value, json};

use super::events_op::Checked;
use super::*;
use crate::app_events::provenance::{self, Invoker, Provenance};

impl ProjectFunctionHost {
    /// The event's id and `PUBLISH_SQL`'s thirteen parameters, in its order:
    /// id, environment, name, key, data, schema revision, actor, causation id,
    /// root id, depth, invocation id, delay seconds, and the provenance MAC
    /// over every one the relay trusts, the row's writer `writer_role` and
    /// the payload's size included (`app_events::provenance`). Every check
    /// has passed by now: the MAC is what tells the relay so.
    pub(super) fn publish_params(
        &self,
        checked: &Checked,
        writer_role: &str,
    ) -> (Uuid, Vec<Value>) {
        let row = self.row_of(checked);
        let mac = provenance::sign_hex(self.org_id, &row.signed(checked, writer_role));
        let params = vec![
            json!(row.id.to_string()),
            json!(row.environment),
            json!(checked.name),
            json!(checked.key),
            checked.data.clone(),
            checked
                .revision
                .map_or(Value::Null, |r| json!(r.to_string())),
            row.actor.clone().map_or(Value::Null, Value::String),
            row.causation
                .map_or(Value::Null, |id| json!(id.to_string())),
            json!(row.root.to_string()),
            json!(row.depth),
            json!(row.invocation.to_string()),
            json!(row.delay),
            json!(mac),
        ];
        (row.id, params)
    }

    /// [`Self::publish_params`] and the three more `PUBLISH_WITH_INVOKER_SQL`
    /// takes: `invoker`'s kind, its token id and the host's invoker MAC over
    /// the same row and the invoker (message domain v2), each `""` when there
    /// is no invoker — set either way, so the definer never reads a value app
    /// SQL left on the connection.
    pub(super) fn publish_params_with_invoker(
        &self,
        checked: &Checked,
        writer_role: &str,
        invoker: Option<Invoker>,
    ) -> (Uuid, Vec<Value>) {
        let (id, mut params) = self.publish_params(checked, writer_role);
        let (kind, token, mac) = match invoker {
            Some(invoker) => {
                let row = self.row_of(checked);
                let signed = row.signed(checked, writer_role);
                (
                    invoker.kind().to_string(),
                    invoker
                        .token_id()
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                    provenance::sign_invoker_hex(self.org_id, &signed, invoker),
                )
            }
            None => Default::default(),
        };
        params.extend([json!(kind), json!(token), json!(mac)]);
        (id, params)
    }

    /// What this run writes on a row for `checked`, beside its own fields.
    fn row_of(&self, checked: &Checked) -> RowOf {
        let environment = self.policy.environment().name();
        let event_type = qualified_type(&self.identity.app_slug, &checked.name);
        let chain = self.chain();
        RowOf {
            id: event_id(self.org_id, &environment, &event_type, &checked.key),
            environment,
            event_type,
            actor: chain.actor,
            root: chain.root,
            causation: chain.causation,
            depth: self.publish_depth(),
            invocation: self.identity.invocation_id,
            // Bounded by the checks: a day of seconds.
            delay: i32::try_from(checked.delay_seconds).unwrap_or(i32::MAX),
        }
    }
}

/// The fields of a row this run writes beyond the publish's own: what
/// [`ProjectFunctionHost::publish_params`] passes and the MACs sign.
struct RowOf {
    id: Uuid,
    environment: String,
    event_type: String,
    actor: Option<String>,
    root: Uuid,
    causation: Option<Uuid>,
    depth: i32,
    invocation: Uuid,
    delay: i32,
}

impl RowOf {
    /// Every field of the row the relay trusts, as the MACs sign them.
    fn signed<'a>(&'a self, checked: &'a Checked, writer_role: &'a str) -> Provenance<'a> {
        Provenance {
            id: self.id,
            environment: &self.environment,
            event_type: &self.event_type,
            key: &checked.key,
            writer_role,
            actor: self.actor.as_deref(),
            root_id: self.root,
            causation_id: self.causation,
            depth: self.depth,
            delay_seconds: self.delay,
            // Bounded by the checks: 64 KiB.
            data_bytes: i64::try_from(checked.data_bytes).unwrap_or(i64::MAX),
            schema_revision: checked.revision,
            invocation_id: Some(self.invocation),
        }
    }
}
