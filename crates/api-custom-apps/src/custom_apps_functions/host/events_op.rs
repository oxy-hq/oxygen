//! `ctx.events.publish` and `tx.events.publish` on the host (app events design
//! §6.2, §6.4): one row in the org's outbox, `oxy_events.outbox`, written
//! through the platform's definer function `oxy_events.publish(...)` as the
//! app's own writer role, which the function records.
//!
//! **Checks, in order, each before anything is written:** the org flag
//! `app_events`; the name among the function's `events.publish`; the name in
//! the running build's `events` catalogue; the key, 1–200 characters and no
//! U+0000; the delay, a whole number of seconds from 0 to 86 400; the
//! payload's size, as `jsonb` text, as the store measures it (the definer
//! function's bound and the relay's are that length,
//! `octet_length(data::text)`); no U+0000 in any of the payload's strings or
//! keys, which Postgres's `text` cannot hold; the payload against the build's
//! schema revision; the depth. Then the environment decides where the
//! row may go — production's store, or the OLTP branch the run (or the
//! handle) was admitted to; held or refused anywhere else (`oltp_guard`) —
//! and the writer is resolved, then granted the definer function
//! (`ensure_publishable`).
//!
//! **Nothing the caller sends names the row.** The id is derived,
//! `event_id(org, environment, <app>/<name>, key)`, the id the relay derives
//! again from the row; the environment is the run's; the root is the cause's
//! root when an event's delivery started the run, else this invocation; the
//! depth is the cause's plus one. On a handle, the transaction is the one the
//! handle names, and its home decides (`EnvPolicy::decide_on_handle`).
//!
//! **Outside a transaction** the row is written in a one-shot transaction on
//! the writer connection, then the org's outbox is marked in the platform
//! (`app_events::marks`) and the relay is queued. **On a `ctx.oltp.tx`
//! handle** it is written on the handle's own connection and the outbox
//! marked at once (a rolled-back publish leaves a harmless mark); when that
//! transaction commits (`tx_finish`) it is marked again — a relay pass that
//! began before the commit could not see the row — and the relay is queued. A
//! publish the server refuses there aborts the app's transaction, as any
//! failed statement on a handle does: it cannot be committed. (The definer's
//! own size refusal, should it ever see a payload the host passed, reaches the
//! app as the host's `EventTooLarge`.) The marks are
//! how the relay sweep finds an outbox without connecting to the org's store.
//! Only a production publish queues a relay: the branch stores are not relayed
//! yet.
//!
//! **A grant this process remembers but the store no longer holds** — the
//! role was re-minted and its grants went with it — is forgotten, granted
//! again (`ensure_publishable`) and the publish tried once more. Outside a
//! transaction that follows a publish refused `42501`, in a new transaction.
//! On a handle a refused statement would abort the app's transaction, so the
//! privilege is read first ([`MAY_PUBLISH_SQL`]), and the publish sent after
//! the grant is in place and visible to the open transaction
//! ([`SEE_NEW_GRANTS`]).

use oxy_custom_apps::events::{
    MAX_DELAY_SECS, MAX_DEPTH, MAX_KEY_LEN, MAX_PAYLOAD_BYTES, event_id, jsonb_text_len,
    qualified_type,
};
use oxy_oltp::entity::tenants::TenantStatus;
use oxy_oltp::events::{EventsError, PUBLISH_SQL, ensure_publishable, forget_publishable};
use oxy_oltp::resolver::WriterConnection;
use serde_json::{Value, json};

use super::super::env_policy::HostOp;
use super::*;

/// What a publish is refused with while the org's `app_events` flag is off.
const NOT_ENABLED: &str = "EventsNotEnabled: app events are not enabled for this org";
/// A key that is missing, not a string, empty or over [`MAX_KEY_LEN`] characters.
const KEY_INVALID: &str = "EventKeyInvalid: `key` must be 1–200 characters";
/// A key holding U+0000, which Postgres's `text` cannot hold.
const KEY_HOLDS_NUL: &str = "EventKeyInvalid: `key` must not contain U+0000";
/// A payload holding U+0000 in a string or an object key, which `jsonb`
/// cannot hold.
const PAYLOAD_HOLDS_NUL: &str =
    "EventPayloadInvalid: `data` must not contain U+0000, in a string or a key";
/// A `delaySeconds` that is not a whole number from 0 to [`MAX_DELAY_SECS`].
const DELAY_INVALID: &str =
    "EventDelayInvalid: `delaySeconds` must be a whole number from 0 to 86400";

/// A `tx.publish` naming a warehouse `ctx.tx` handle: only the app's own OLTP
/// store has an outbox.
const NOT_AN_OLTP_HANDLE: &str =
    "`id` must be a ctx.oltp.tx handle: an event is published in the app's own OLTP store";

/// Whether the session's role may publish: `USAGE` on the events schema and
/// `EXECUTE` on its definer function, read from the catalogs so the read
/// itself never fails (naming the function would, without `USAGE`). `false`
/// when the store has no such function.
const MAY_PUBLISH_SQL: &str = "SELECT coalesce(bool_and(\
     has_schema_privilege(n.oid, 'USAGE') AND has_function_privilege(p.oid, 'EXECUTE')\
     ), false) AS may_publish \
     FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
     WHERE n.nspname = 'oxy_events' AND p.proname = 'publish'";

/// A read that locks a catalog no app statement touches, which makes the
/// handle's session take in the catalog changes other sessions committed since
/// its transaction began — the grant just made again among them. Without it
/// the publish on the same transaction still sees the privilege as it was when
/// the transaction first looked, and is refused.
const SEE_NEW_GRANTS: &str = "SELECT 1 FROM pg_catalog.pg_seclabel LIMIT 0";

// The host's payload limit is the definer function's: a payload the host
// passes is one the store accepts, and so one the relay passes.
const _: () = assert!(oxy_oltp::platform::EVENT_MAX_DATA_TEXT_BYTES as usize == MAX_PAYLOAD_BYTES);

/// A publish that passed every check on its arguments.
struct Checked {
    name: String,
    key: String,
    data: Value,
    delay_seconds: i64,
    /// The schema revision of the running build's declaration, which the data
    /// was validated against.
    revision: Uuid,
}

impl ProjectFunctionHost {
    /// `ctx.events.publish(type, data, { key, delaySeconds? })`: the row in its
    /// own transaction, then the relay. See the module docs.
    pub(super) async fn events_op(
        &self,
        op: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let host_op = HostOp::sub_op("events", op).ok_or_else(|| format!("unknown op '{op}'"))?;
        if host_op != HostOp::EventsPublish {
            return Err(format!("unknown op '{op}'"));
        }
        let checked = self.check_publish(host_op, &payload).await?;
        let decided = self.policy.decide(host_op);
        self.admit_publish(host_op, decided, &checked.name).await?;
        let conn = self.publish_writer(host_op).await?;
        let (id, params) = self.publish_params(&checked);
        self.publish_alone(&conn, &params).await?;
        self.after_publish_commit().await;
        Ok(json!({ "eventId": id.to_string() }))
    }

    /// `tx.events.publish(type, data, { key, delaySeconds? })` on the open
    /// `ctx.oltp.tx` handle `payload.id`: the row on the handle's own
    /// connection, the relay when it commits. See the module docs.
    pub(super) async fn tx_publish(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let host_op = HostOp::TxPublish;
        let id = payload
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| "`id` is required".to_string())?;
        let checked = self.check_publish(host_op, &payload).await?;
        let Some((plane, opened_into)) = self
            .tx_writes
            .lock()
            .await
            .get(&id)
            .map(|audit| (audit.plane, audit.opened_into))
        else {
            return Err(super::super::tx::unknown_handle());
        };
        if plane != "oltp" {
            return Err(NOT_AN_OLTP_HANDLE.to_string());
        }
        // The handle's home, never the run's: the publish runs on the
        // handle's connection, wherever its `begin` went.
        let decided = self.policy.decide_on_handle(host_op, opened_into);
        self.admit_publish(host_op, decided, &checked.name).await?;
        let conn = self.publish_writer(host_op).await?;
        let (event, params) = self.publish_params(&checked);
        self.publish_on_handle(id, &conn, &params).await?;
        if let Some(audit) = self.tx_writes.lock().await.get_mut(&id) {
            audit.published = true;
        }
        self.mark_published().await;
        Ok(json!({ "eventId": event.to_string() }))
    }

    /// The checks of the module docs, in that order, on the payload's
    /// named fields; anything else in it is ignored.
    async fn check_publish(&self, op: HostOp, payload: &Value) -> Result<Checked, String> {
        use oxy_app::surface::feature_flags::{APP_EVENTS, FlagContext, is_enabled_for};
        if !is_enabled_for(APP_EVENTS, &FlagContext::org(self.org_id)) {
            return Err(NOT_ENABLED.to_string());
        }
        let name = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let publishable = self.caps.events.as_ref().ok_or_else(|| gate_bypassed(op))?;
        if !publishable.contains(name) {
            let names: Vec<&str> = publishable.iter().map(String::as_str).collect();
            return Err(format!(
                "EventNotPublishable: this function may publish [{}], not `{name}`",
                names.join(", ")
            ));
        }
        let Some(declared) = self.caps.event_catalog.events.get(name) else {
            return Err(format!(
                "EventNotDeclared: `{name}` is not in this app's `events`"
            ));
        };
        let key = match payload.get("key") {
            Some(Value::String(key)) if (1..=MAX_KEY_LEN).contains(&key.chars().count()) => {
                key.clone()
            }
            _ => return Err(KEY_INVALID.to_string()),
        };
        if key.contains('\0') {
            return Err(KEY_HOLDS_NUL.to_string());
        }
        let delay_seconds = delay_seconds(payload.get("delaySeconds"))?;
        let data = payload.get("data").cloned().unwrap_or(Value::Null);
        // As `jsonb` text, exactly as the store measures it: the definer
        // function refuses a longer payload and the relay quarantines one
        // (`relay::oversized`), each by `octet_length(data::text)`. Refused
        // here, before anything is sent, a `tx` handle stays usable; refused
        // by the definer, its transaction would be aborted.
        let bytes = jsonb_text_len(&data);
        if bytes > MAX_PAYLOAD_BYTES {
            return Err(format!(
                "EventTooLarge: payload is {bytes} bytes; the limit is {MAX_PAYLOAD_BYTES}"
            ));
        }
        if holds_nul(&data) {
            return Err(PAYLOAD_HOLDS_NUL.to_string());
        }
        crate::app_events::schema::validate(&declared.schema, &data)
            .map_err(|errors| format!("EventSchemaMismatch: {}", errors.join("; ")))?;
        let revision = crate::app_events::schema::recorded_revision(
            &self.db,
            self.app_id,
            name,
            &declared.schema,
        )
        .await
        .map_err(|e| format!("events: could not read the schema revision of `{name}`: {e}"))?
        .ok_or_else(|| {
            format!(
                "events: this build's schema for `{name}` has no recorded revision; publish \
                 the app again"
            )
        })?;
        let depth = self.publish_depth();
        if depth > MAX_DEPTH {
            return Err(format!(
                "EventDepthExceeded: this event would be depth {depth}; the limit is {MAX_DEPTH}"
            ));
        }
        Ok(Checked {
            name: name.to_string(),
            key,
            data,
            delay_seconds,
            revision: revision.id,
        })
    }

    /// 0 for a publish no event's handler made; the cause's depth plus one
    /// otherwise.
    fn publish_depth(&self) -> i32 {
        self.event_cause
            .as_ref()
            .map_or(0, |cause| cause.depth.saturating_add(1))
    }

    /// The writer a publish is written as, resolved on the run's OLTP home —
    /// the writer first, so a missing writer role says so in `ctx.oltp`'s
    /// words, and an org with no OLTP store at all is told how to get one —
    /// then granted the definer function.
    async fn publish_writer(&self, op: HostOp) -> Result<WriterConnection, String> {
        let writer = match &self.caps.events_writer {
            WriterCapability::Enabled { writer } => writer.as_str(),
            WriterCapability::SlugNotDerivable { slug } => {
                return Err(slug_cannot_back_a_schema("OLTP", slug));
            }
            WriterCapability::Disabled => return Err(gate_bypassed(op)),
        };
        let conn = match self.oltp_connection(writer).await {
            Ok(conn) => conn,
            Err(e) if self.lacks_an_oltp_store().await => {
                tracing::debug!(error = %e, "app events: the org has no OLTP store");
                return Err(EventsError::NotProvisioned {
                    org_id: self.org_id,
                }
                .to_string());
            }
            Err(e) => return Err(e),
        };
        with_db_timeout("grant", async {
            ensure_publishable(&self.db, self.org_id, &conn.role)
                .await
                .map_err(|e| e.to_string())
        })
        .await?;
        Ok(conn)
    }

    /// Whether the org has no OLTP store an operator could use: none, or one
    /// whose provision failed or is being deleted (`EventsError::NotProvisioned`
    /// says the same).
    async fn lacks_an_oltp_store(&self) -> bool {
        match oxy_oltp::migrator::tenant_for_org(&self.db, self.org_id).await {
            Ok(tenant) => tenant.status != TenantStatus::Active,
            Err(oxy_oltp::migrator::MigrateError::NotProvisioned(_)) => true,
            Err(_) => false,
        }
    }

    /// The event's id and [`PUBLISH_SQL`]'s twelve parameters, in its order:
    /// id, environment, name, key, data, schema revision, actor, causation id,
    /// root id, depth, invocation id, delay seconds.
    fn publish_params(&self, checked: &Checked) -> (Uuid, Vec<Value>) {
        let environment = self.policy.environment().name();
        let event_type = qualified_type(&self.identity.app_slug, &checked.name);
        let id = event_id(self.org_id, &environment, &event_type, &checked.key);
        let invocation = self.identity.invocation_id;
        let cause = self.event_cause.as_ref();
        // The person whose action started the chain: the cause's, or this
        // run's caller when a person called it. Attribution only.
        let actor = match cause {
            Some(cause) => cause.actor.clone(),
            None => self.identity.user_id.map(|user| user.to_string()),
        };
        let root = cause.map_or(invocation, |cause| cause.root_id);
        let params = vec![
            json!(id.to_string()),
            json!(environment),
            json!(checked.name),
            json!(checked.key),
            checked.data.clone(),
            json!(checked.revision.to_string()),
            actor.map_or(Value::Null, Value::String),
            cause.map_or(Value::Null, |cause| json!(cause.id.to_string())),
            json!(root.to_string()),
            json!(self.publish_depth()),
            json!(invocation.to_string()),
            json!(checked.delay_seconds),
        ];
        (id, params)
    }

    /// The publish in a one-shot transaction of its own on `conn`.
    async fn publish_alone(&self, conn: &WriterConnection, params: &[Value]) -> Result<(), String> {
        let summary = db_query_summary(PUBLISH_SQL);
        let (_, traceparent) = Self::trace_context();
        let tagged = data_audit::commented(PUBLISH_SQL, &self.identity, traceparent.as_deref());
        let mut regranted = false;
        loop {
            let mut tx = self
                .open_statement_tx(HostOp::EventsPublish, conn, &summary)
                .await?;
            let sent = with_db_timeout("publish", async {
                tx.exec(&tagged, params).await.map_err(|e| e.to_string())
            })
            .await;
            match sent {
                Ok(_) => {
                    return with_db_timeout("commit", async move {
                        tx.commit().await.map_err(|e| format!("commit failed: {e}"))
                    })
                    .await;
                }
                Err(e) => {
                    // A rollback failure is moot: the dropped connection rolls
                    // back anyway, and it must not mask the real error.
                    let _ = tx.rollback().await;
                    if regranted || !is_insufficient_privilege(&e) {
                        return Err(definer_size_refusal(&e).unwrap_or(e));
                    }
                    self.grant_again(&conn.role).await?;
                    regranted = true;
                }
            }
        }
    }

    /// The publish on handle `id`'s own connection. The privilege is read
    /// first: a grant this process remembers but the store lost is made again
    /// before anything is sent, so a stale grant never aborts the app's
    /// transaction.
    async fn publish_on_handle(
        &self,
        id: u64,
        conn: &WriterConnection,
        params: &[Value],
    ) -> Result<(), String> {
        let summary = db_query_summary(PUBLISH_SQL);
        data_audit::record_db_span(&summary, Some(&conn.schema), None);
        let rows =
            with_db_timeout("publish", self.transactions.query(id, MAY_PUBLISH_SQL, &[])).await?;
        let may_publish = rows
            .first()
            .and_then(|row| row.get("may_publish"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !may_publish {
            self.grant_again(&conn.role).await?;
            self.on_handle(id, SEE_NEW_GRANTS, &[]).await?;
        }
        let (_, traceparent) = Self::trace_context();
        let tagged = data_audit::commented(PUBLISH_SQL, &self.identity, traceparent.as_deref());
        self.on_handle(id, &tagged, params)
            .await
            .map_err(|e| definer_size_refusal(&e).unwrap_or(e))
    }

    /// One statement on handle `id`, bounded like every other.
    async fn on_handle(&self, id: u64, sql: &str, params: &[Value]) -> Result<(), String> {
        with_db_timeout("publish", self.transactions.exec(id, sql, params))
            .await
            .map(|_| ())
    }

    /// Forget the remembered grant of `role` and grant it again.
    async fn grant_again(&self, role: &str) -> Result<(), String> {
        forget_publishable(self.org_id, role);
        with_db_timeout("grant", async {
            ensure_publishable(&self.db, self.org_id, role)
                .await
                .map_err(|e| e.to_string())
        })
        .await
    }

    /// After a publish's row has committed: mark the outbox again, then queue
    /// its relay. Neither is thrown: the row is committed.
    pub(super) async fn after_publish_commit(&self) {
        self.mark_published().await;
        self.nudge_relay().await;
    }

    /// Mark this environment's outbox published, for the relay sweep. Logged,
    /// never thrown: the publish stands, and its nudge still queues a relay;
    /// only the sweep's second chance waits for the next mark.
    async fn mark_published(&self) {
        let environment = self.policy.environment().name();
        if let Err(e) =
            crate::app_events::marks::mark_published(&self.db, self.org_id, &environment).await
        {
            tracing::warn!(
                org_id = %self.org_id,
                environment,
                error = %e,
                "app events: could not mark the outbox after a publish"
            );
        }
    }

    /// Queue a relay of this environment's outbox. The row is committed, so a
    /// failure here only delays it until the next sweep: logged, never thrown.
    ///
    /// Production only: the branch stores staging and the sandboxes publish
    /// into are not relayed yet (Task 17), and a relay queued for one could
    /// only end as a failed run. Their publishes are marked all the same.
    async fn nudge_relay(&self) {
        let environment = self.policy.environment().name();
        if crate::app_events::relay::relayed_store(&environment).is_none() {
            return;
        }
        if let Err(e) =
            crate::app_events::relay::enqueue_relay(&self.db, self.org_id, &environment).await
        {
            tracing::warn!(
                org_id = %self.org_id,
                environment,
                error = %e,
                "app events: could not queue the relay after a publish"
            );
        }
    }
}

/// `delaySeconds`: absent (or `null`) is 0; otherwise a whole number of
/// seconds from 0 to [`MAX_DELAY_SECS`] — `30.0` is whole, `1.5` and `"30"`
/// are not.
fn delay_seconds(value: Option<&Value>) -> Result<i64, String> {
    let seconds = match value {
        None | Some(Value::Null) => return Ok(0),
        Some(Value::Number(n)) => match (n.as_u64(), n.as_f64()) {
            (Some(whole), _) => Some(whole),
            (None, Some(f)) if f.fract() == 0.0 && f >= 0.0 && f <= MAX_DELAY_SECS as f64 => {
                Some(f as u64)
            }
            _ => None,
        },
        Some(_) => None,
    };
    match seconds {
        Some(seconds) if seconds <= MAX_DELAY_SECS => Ok(seconds as i64),
        _ => Err(DELAY_INVALID.to_string()),
    }
}

/// Whether `value` holds U+0000 in any string or object key, at any depth.
/// Postgres's `text`, and so `jsonb`, cannot hold it (`22P05`): refused by
/// the store, the publish would abort a `tx` handle, so the host refuses it
/// first.
fn holds_nul(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains('\0'),
        Value::Array(items) => items.iter().any(holds_nul),
        Value::Object(members) => members
            .iter()
            .any(|(key, value)| key.contains('\0') || holds_nul(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// Postgres refused the statement for a missing privilege (SQLSTATE 42501),
/// as the connector writes it (`query failed: [42501] …`).
fn is_insufficient_privilege(error: &str) -> bool {
    error.contains("[42501]")
}

/// The definer function's size refusal (SQLSTATE 22023, "data is N bytes as
/// jsonb text; the limit is L"), as the connector writes it, in the host's own
/// words; `None` for any other error.
///
/// The host measures a payload as the store does ([`jsonb_text_len`]) and
/// refuses an oversized one before anything is sent, so this is only reached
/// should the two ever disagree: the app is told the same `EventTooLarge` it
/// would have been told up front (a caller error, which never pages), with the
/// store's figures. On a `tx` handle the refused statement has already
/// aborted the app's transaction, as any refused statement there does.
fn definer_size_refusal(error: &str) -> Option<String> {
    if !error.starts_with("query failed: [22023] ") {
        return None;
    }
    let (_, rest) = error.split_once("oxy_events.publish: data is ")?;
    let (bytes, rest) = rest.split_once(" bytes as jsonb text; the limit is ")?;
    let limit: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let bytes: u64 = bytes.parse().ok()?;
    let limit: u64 = limit.parse().ok()?;
    Some(format!(
        "EventTooLarge: payload is {bytes} bytes; the limit is {limit}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_is_a_whole_number_of_seconds_within_a_day() {
        for (value, expected) in [
            (None, Ok(0)),
            (Some(json!(null)), Ok(0)),
            (Some(json!(0)), Ok(0)),
            (Some(json!(30)), Ok(30)),
            (Some(json!(30.0)), Ok(30)),
            (Some(json!(86_400)), Ok(86_400)),
        ] {
            assert_eq!(delay_seconds(value.as_ref()), expected, "{value:?}");
        }
        for value in [
            json!(-1),
            json!(1.5),
            json!(86_401),
            json!(86_400.5),
            json!("30"),
            json!(true),
            json!(-0.5),
        ] {
            assert_eq!(
                delay_seconds(Some(&value)),
                Err(DELAY_INVALID.to_string()),
                "{value}"
            );
        }
    }

    /// R32: the definer function holds a payload to the relay's limit by the
    /// store's own measure. When it refuses one the host passed, the app is
    /// told `EventTooLarge` in the host's words — its own error, which pages
    /// nobody — never the store's text. Every other refusal stays as it was.
    #[test]
    fn the_definers_size_refusal_is_event_too_large() {
        use super::super::super::host_call_attrs::{classify_host_error, counts_toward_paging};
        let refused = "query failed: [22023] oxy_events.publish: data is 75812 bytes as jsonb \
                       text; the limit is 65536\nSQL: SELECT oxy_events.publish($1,$2)";
        assert!(
            counts_toward_paging(classify_host_error(refused)),
            "the store's own text would page"
        );
        let mapped = definer_size_refusal(refused).expect("the size refusal");
        assert_eq!(
            mapped,
            "EventTooLarge: payload is 75812 bytes; the limit is 65536"
        );
        let kind = classify_host_error(&mapped);
        assert_eq!(kind, "bad_request");
        assert!(!counts_toward_paging(kind));
        for other in [
            "query failed: [22023] oxy_events.publish: depth 9 is outside 0 to 8\nSQL: SELECT 1",
            "query failed: [22023] oxy_events.publish: data is <NULL> bytes as jsonb text; the \
             limit is 65536\nSQL: SELECT 1",
            "query failed: [42501] permission denied for function publish\nSQL: SELECT 1",
            "query failed: [XX000] oxy_events.publish: data is 75812 bytes as jsonb text; the \
             limit is 65536\nSQL: SELECT 1",
            "connection error: closed",
        ] {
            assert_eq!(definer_size_refusal(other), None, "{other}");
        }
    }

    /// Follow-up to R32: Postgres's `text`, and so `jsonb`, cannot hold
    /// U+0000. A payload holding it anywhere — a string at any depth, or an
    /// object key — is the app's own error, refused before the statement
    /// (which would abort a `tx` handle), and pages nobody.
    #[test]
    fn a_payload_holding_nul_anywhere_is_found() {
        use super::super::super::host_call_attrs::{classify_host_error, counts_toward_paging};
        for held in [
            json!("a\u{0}b"),
            json!({ "a": ["x", { "b": "\u{0}" }] }),
            json!({ "k\u{0}": 1 }),
            json!([[[{ "deep": [null, "\u{0}"] }]]]),
        ] {
            assert!(holds_nul(&held), "{held}");
        }
        for clean in [
            json!({ "a": "\\u0000 is six characters here", "b": [1, true, null] }),
            json!("é😀\u{1}\u{7f}"),
            json!({}),
            json!(0),
        ] {
            assert!(!holds_nul(&clean), "{clean}");
        }
        for refusal in [PAYLOAD_HOLDS_NUL, KEY_HOLDS_NUL] {
            let kind = classify_host_error(refusal);
            assert_eq!(kind, "bad_request", "{refusal}");
            assert!(!counts_toward_paging(kind), "{refusal}");
        }
    }

    #[test]
    fn only_a_privilege_refusal_is_retried() {
        assert!(is_insufficient_privilege(
            "query failed: [42501] permission denied for function publish\nSQL: SELECT …"
        ));
        assert!(!is_insufficient_privilege(
            "query failed: [23505] duplicate key value violates unique constraint"
        ));
        assert!(!is_insufficient_privilege("connection error: closed"));
    }
}
