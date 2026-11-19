//! **The single declaration of what every tenant database must contain.**
//!
//! Database-per-tenant means Oxy owns objects inside N databases — the ledger
//! schema, the analyst role, baseline grants — and every change to them is a
//! fan-out. The tax is unavoidable; what makes it survivable is that the
//! expected state is declared in exactly one place, versioned, and recorded
//! per tenant in Oxy's *own* database.
//!
//! That last part is what lets Oxy answer "which tenants are behind?" with a
//! single query against `oltp_tenants.platform_schema_version`, rather than
//! connecting to every tenant to find out.
//!
//! # Rules for adding a step
//!
//! 1. **Append only.** Never edit a shipped step — tenants that already ran it
//!    will not run it again, so an edit silently applies to new tenants only.
//! 2. **Idempotent.** Every statement must tolerate being re-run (`IF NOT
//!    EXISTS`, or a `GRANT` that is already held). Reconcile then repairs drift
//!    rather than only filling gaps.
//! 3. **Bump [`PLATFORM_SCHEMA_VERSION`].** A test fails if it doesn't match
//!    the step count, so this can't be forgotten.
//!
//! Note the asymmetry with app migrations ([`crate::schema`] docs): those are
//! forward-only because reverting a build can't un-drop a column. These are
//! idempotent *and* forward-only, because a tenant may be many versions behind.

use crate::schema::{SchemaError, analyst_role_for, validate_name};

/// Version of the Oxy-owned objects a tenant database is expected to carry.
///
/// Recorded on `oltp_tenants.platform_schema_version`. A tenant below this is
/// reconciled the next time anything touches it.
pub const PLATFORM_SCHEMA_VERSION: i32 = 7;

/// Table recording which workspace migrations a tenant has applied.
///
/// Distinct from the earlier `app_migrations`: schemas are authored per
/// **workspace** and keyed by `file_path`, not per app. `app_migrations`
/// survives because platform steps are append-only, but nothing writes it.
pub const MIGRATIONS_TABLE: &str = "oxy_meta.schema_migrations";

/// Schema holding Oxy-owned bookkeeping inside a tenant database.
///
/// Deliberately **not** owned by any writer: an app owns `app_<slug>` and can
/// do as it likes there, but it must not be able to rewrite its own migration
/// history.
pub const META_SCHEMA: &str = "oxy_meta";

/// Schema holding the org's notification inbox — what each notification says.
///
/// Oxy-owned like [`META_SCHEMA`], and for the same reason not owned by any
/// writer: a custom app sends notifications through `ctx.notify`, it does not
/// get to rewrite the inbox. Content lives here, in the org's own database,
/// because it is the org's data; Oxy's database keeps only who was told and
/// whether they read it (`notifications.message_id` points here). The org
/// takes these rows with it if it leaves. Design:
/// `internal-docs/custom-app-notifications.md`.
pub const INBOX_SCHEMA: &str = "oxy_inbox";

/// The inbox content table, schema-qualified.
pub const INBOX_MESSAGES_TABLE: &str = "oxy_inbox.messages";

/// Schema holding the org's events outbox — events apps published, waiting
/// for the relay, and once relayed the log their bodies are read from.
///
/// Oxy-owned like [`INBOX_SCHEMA`], but unlike it reached by apps' writer
/// roles, inside their own transactions, so a publish commits or rolls back
/// with the app's write. The reach is one function, [`EVENTS_PUBLISH_FUNCTION`]:
/// a writer holds `USAGE` here and `EXECUTE` on that, never a privilege on the
/// table, so it cannot read other apps' events or publish as another app.
/// Design: `internal-docs/2026-11-17-app-events-design.md` §6.2.
pub const EVENTS_SCHEMA: &str = "oxy_events";

/// The outbox table, schema-qualified. A row is waiting while its
/// `relayed_at` is null; relayed, it stays as the event body's log (the
/// platform keeps only the event's routing metadata) until retention deletes
/// it with the event.
pub const EVENTS_OUTBOX_TABLE: &str = "oxy_events.outbox";

/// The deepest causation chain the definer function accepts (an event no
/// handler published is depth 0) — the platform's limit, also the host's.
pub const EVENT_MAX_DEPTH: i32 = 8;
/// The longest `delay_seconds` the definer function accepts (24 hours).
pub const EVENT_MAX_DELAY_SECONDS: i32 = 86_400;
/// The longest key, in characters, the definer function accepts.
pub const EVENT_MAX_KEY_CHARS: i32 = 200;
/// The largest payload, as `jsonb` text (`octet_length(p_data::text)`), the
/// definer function accepts: the relay's own limit, read by the relay with
/// the same expression on the stored row, so a row the store accepts is
/// always one the relay passes. The host measures a payload the same way
/// before it sends one, and refuses a larger one itself; this bound is what
/// holds a writer's own SQL, and any payload the host's measure misjudged,
/// to it. The host reports its refusal (`invalid_parameter_value`, "data is
/// N bytes") as the app's own `EventTooLarge`.
pub const EVENT_MAX_DATA_TEXT_BYTES: i64 = 65_536;

/// The definer function writers publish through, by its full signature — the
/// form `GRANT` and `REVOKE` name it by. Platform step 6's: the twelve
/// arguments step 5 took, then the host's provenance MAC.
pub const EVENTS_PUBLISH_FUNCTION: &str =
    "oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int,bytea)";

/// The definer function as platform step 5 created it, without the
/// provenance MAC. Step 6 drops it, so a server still calling it fails
/// rather than writing a row nothing signed.
pub(crate) const EVENTS_PUBLISH_FUNCTION_V5: &str =
    "oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int)";

/// The length of the provenance MAC the definer function takes and stores
/// (`HMAC-SHA256`), and the only length it accepts.
pub const EVENT_PROVENANCE_BYTES: i32 = 32;

/// The transaction-local setting a publish statement passes the send's
/// invoker kind through (`session` or `token`), which the definer reads once
/// and clears (platform step 7). A setting, not an argument: the definer's
/// signature is step 6's, so a server one release back keeps calling it.
pub const EVENT_INVOKER_SETTING: &str = "oxy_events.invoker";
/// The same, for the invoker's stored token id (`token` only).
pub const EVENT_INVOKER_TOKEN_SETTING: &str = "oxy_events.invoker_token_id";
/// The same, for the host's MAC over the row and the invoker, as hex.
pub const EVENT_INVOKER_PROVENANCE_SETTING: &str = "oxy_events.invoker_provenance";

// The events outbox's later steps.
mod outbox;

/// One versioned change to the Oxy-owned state of a tenant database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformStep {
    /// The version a tenant is at once this step has been applied.
    pub version: i32,
    pub description: &'static str,
    pub statements: Vec<String>,
}

/// Every step, in order. `steps[i].version == i + 1`.
/// `provider` is not decoration: on a cluster where tenants **share a role
/// namespace** (`local`), every role name carries a per-tenant tag, so the
/// analyst here is `oxy_analyst_ro_<tag>` and the bare `oxy_analyst_ro` is a
/// decoy. Hardening the bare name meant `REVOKE CREATE ON SCHEMA public` and
/// `REVOKE ALL ON SCHEMA oxy_meta` landed on a role nothing logs in as, while
/// the role that actually serves queries kept whatever it was created with.
pub fn platform_steps(provider: &str, database: &str) -> Result<Vec<PlatformStep>, SchemaError> {
    validate_name(database)?;
    let db = quote(database);
    let meta = quote(META_SCHEMA);
    let inbox = quote(INBOX_SCHEMA);
    let analyst_name = analyst_role_for(provider, database);
    let analyst = quote(&analyst_name);

    Ok(vec![
        PlatformStep {
            version: 1,
            description: "baseline hardening, oxy_meta ledger, analyst role",
            statements: vec![
                // ── Baseline hardening ──────────────────────────────────────────
                // Postgres 15+ already revokes CREATE on public from PUBLIC, but
                // stating it explicitly means the invariant holds regardless of the
                // provider's template or server version.
                format!("REVOKE ALL ON DATABASE {db} FROM PUBLIC"),
                "REVOKE CREATE ON SCHEMA public FROM PUBLIC".to_string(),
                // ── Oxy-owned bookkeeping ───────────────────────────────────────
                format!("CREATE SCHEMA IF NOT EXISTS {meta}"),
                format!(
                    "CREATE TABLE IF NOT EXISTS {meta}.app_migrations (
                     app_id      UUID        NOT NULL,
                     version     INTEGER     NOT NULL,
                     name        TEXT        NOT NULL,
                     checksum    TEXT        NOT NULL,
                     applied_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                     PRIMARY KEY (app_id, version)
                 )"
                ),
                // ── Analyst role ────────────────────────────────────────────────
                // NOLOGIN: the provider mints the login credential. Created with no
                // schema grants at all, so a fresh database exposes nothing until a
                // writer is explicitly published to analytics.
                format!(
                    "DO $$ BEGIN \
                   IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{analyst_name}') THEN \
                     CREATE ROLE {analyst} NOLOGIN; \
                   END IF; \
                 END $$"
                ),
                format!("REVOKE CREATE ON SCHEMA public FROM {analyst}"),
                // The analyst must never see Oxy's own bookkeeping.
                format!("REVOKE ALL ON SCHEMA {meta} FROM {analyst}"),
            ],
        },
        PlatformStep {
            version: 2,
            description: "workspace schema-migration ledger",
            statements: vec![format!(
                "CREATE TABLE IF NOT EXISTS {meta}.schema_migrations (
                 file_path   TEXT        NOT NULL PRIMARY KEY,
                 checksum    TEXT        NOT NULL,
                 revision_id UUID,
                 applied_at  TIMESTAMPTZ NOT NULL DEFAULT now()
             )"
            )],
        },
        PlatformStep {
            version: 3,
            description: "harden the tenant-qualified analyst role",
            // Step 1 built these statements against the BARE `oxy_analyst_ro`,
            // so every tenant provisioned before this ran them on a decoy. The
            // statements are identical and idempotent; a tenant provisioned
            // after the fix runs them a second time and nothing changes.
            //
            // A new step rather than an edit to step 1: a tenant already at
            // version 2 never re-runs step 1, so editing it would fix new
            // tenants only — which is the half that was never broken.
            statements: vec![
                format!(
                    "DO $$ BEGIN \
                   IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{analyst_name}') THEN \
                     CREATE ROLE {analyst} NOLOGIN; \
                   END IF; \
                 END $$"
                ),
                format!("REVOKE CREATE ON SCHEMA public FROM {analyst}"),
                format!("REVOKE ALL ON SCHEMA {meta} FROM {analyst}"),
            ],
        },
        PlatformStep {
            version: 4,
            description: "notification inbox content",
            // One row per notification sent, however many people it went to:
            // the per-recipient rows (and their read state) are signals in
            // Oxy's database, keyed by this id. Written and read as the owner.
            //
            // No foreign keys and no `app_id` reference: the app registry is
            // Oxy's, not the tenant's. The analyst does not see it — an inbox
            // is addressed to people, and publishing it to analytics is the
            // org's call, not a default.
            statements: vec![
                format!("CREATE SCHEMA IF NOT EXISTS {inbox}"),
                format!(
                    "CREATE TABLE IF NOT EXISTS {inbox}.messages (
                     id            UUID        PRIMARY KEY,
                     app_id        UUID,
                     kind          TEXT        NOT NULL,
                     title         TEXT        NOT NULL,
                     body          TEXT,
                     link          TEXT,
                     email_subject TEXT,
                     email_html    TEXT,
                     email_text    TEXT,
                     created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
                 )"
                ),
                format!("REVOKE ALL ON SCHEMA {inbox} FROM {analyst}"),
            ],
        },
        PlatformStep {
            version: 5,
            description: "events outbox, written through a definer function",
            // The definer function is the security boundary. It runs as the
            // owner, who owns the table, and records `session_user` — the
            // writer role that logged in, which `SECURITY DEFINER` does not
            // change — so an app cannot publish as another. `search_path` is
            // pinned and every name in the body qualified, so a caller cannot
            // put its own objects in the function's way.
            //
            // Postgres grants `EXECUTE` on a new function to `PUBLIC`; it is
            // revoked here, and each writer is granted it (with `USAGE` on the
            // schema, nothing on the table) by `events::ensure_publishable`.
            // `CREATE OR REPLACE` keeps those grants on a re-run.
            //
            // Keyed on `(writer_role, id)`, not the id alone: a writer chooses
            // the id it passes, so with the id as the key one app could insert
            // the id of another app's event before it is published and turn
            // that publish into a silent `ON CONFLICT` no-op. A repeat stays a
            // no-op within one writer's own ids; the relay derives the id it
            // stores an event under.
            //
            // Read by the relay as the owner, which sets `relayed_at` rather
            // than deleting: the platform stores the event's routing metadata
            // and never its body, so the relayed row is the body's log, read
            // by `(writer_role, id)` when the event is delivered and deleted
            // by retention with the event. A repeat of a relayed event is
            // still the `ON CONFLICT` no-op. The claim reads only waiting
            // rows, oldest first, so its index holds only those. The analyst
            // does not see the table: these are apps' messages to each other,
            // not the org's data to analyse.
            //
            // A writer can call the function from its own SQL as well as
            // through the host, with any arguments, so the bounds every
            // honest publish is held to are checked here too, before the
            // insert: depth, delay, key length, and the payload's size as
            // `jsonb` text — 64 KiB, by the very expression the relay reads,
            // so no row is stored that the relay would then refuse. Each
            // refuses `invalid_parameter_value` naming the field.
            statements: vec![
                format!("CREATE SCHEMA IF NOT EXISTS {EVENTS_SCHEMA}"),
                format!(
                    "CREATE TABLE IF NOT EXISTS {EVENTS_OUTBOX_TABLE} (
  id uuid NOT NULL, writer_role text NOT NULL, environment text NOT NULL,
  name text NOT NULL, key text NOT NULL, data jsonb NOT NULL, schema_revision uuid,
  actor text, causation_id uuid, root_id uuid NOT NULL, depth int NOT NULL DEFAULT 0, invocation_id uuid,
  delay_seconds int NOT NULL DEFAULT 0, created_at timestamptz NOT NULL DEFAULT now(),
  relayed_at timestamptz,
  PRIMARY KEY (writer_role, id))"
                ),
                format!(
                    "CREATE INDEX IF NOT EXISTS outbox_unrelayed ON {EVENTS_OUTBOX_TABLE} (created_at) \
                     WHERE relayed_at IS NULL"
                ),
                format!(
                    "CREATE OR REPLACE FUNCTION {EVENTS_SCHEMA}.publish(
  p_id uuid, p_environment text, p_name text, p_key text, p_data jsonb, p_schema_revision uuid,
  p_actor text, p_causation_id uuid, p_root_id uuid, p_depth int, p_invocation_id uuid, p_delay_seconds int)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
BEGIN
  IF p_depth IS NULL OR p_depth < 0 OR p_depth > {EVENT_MAX_DEPTH} THEN
    RAISE EXCEPTION 'oxy_events.publish: depth % is outside 0 to {EVENT_MAX_DEPTH}', p_depth
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF p_delay_seconds IS NULL OR p_delay_seconds < 0 OR p_delay_seconds > {EVENT_MAX_DELAY_SECONDS} THEN
    RAISE EXCEPTION 'oxy_events.publish: delay_seconds % is outside 0 to {EVENT_MAX_DELAY_SECONDS}', p_delay_seconds
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF p_key IS NULL OR char_length(p_key) < 1 OR char_length(p_key) > {EVENT_MAX_KEY_CHARS} THEN
    RAISE EXCEPTION 'oxy_events.publish: key is % characters; it must be 1 to {EVENT_MAX_KEY_CHARS}', char_length(p_key)
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  IF p_data IS NULL OR octet_length(p_data::text) > {EVENT_MAX_DATA_TEXT_BYTES} THEN
    RAISE EXCEPTION 'oxy_events.publish: data is % bytes as jsonb text; the limit is {EVENT_MAX_DATA_TEXT_BYTES}', octet_length(p_data::text)
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  INSERT INTO {EVENTS_OUTBOX_TABLE} (id, writer_role, environment, name, key, data, schema_revision,
    actor, causation_id, root_id, depth, invocation_id, delay_seconds)
  VALUES (p_id, session_user, p_environment, p_name, p_key, p_data, p_schema_revision,
    p_actor, p_causation_id, p_root_id, p_depth, p_invocation_id, p_delay_seconds)
  ON CONFLICT (writer_role, id) DO NOTHING;
  RETURN p_id;
END
$$"
                ),
                format!("REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION_V5} FROM PUBLIC"),
                format!("REVOKE ALL ON SCHEMA {EVENTS_SCHEMA} FROM {analyst}"),
            ],
        },
        outbox::provenance_step(),
        outbox::invoker_step(),
    ])
}

/// Steps needed to bring a tenant from `from_version` up to current.
///
/// `from_version` of 0 means a freshly provisioned database, which therefore
/// gets the whole list — a new tenant is correct by construction rather than by
/// remembering to run something extra at provision time.
pub fn steps_since(
    from_version: i32,
    provider: &str,
    database: &str,
) -> Result<Vec<PlatformStep>, SchemaError> {
    Ok(platform_steps(provider, database)?
        .into_iter()
        .filter(|s| s.version > from_version)
        .collect())
}

/// Flattened statements for [`steps_since`], ready for a
/// [`crate::sql::TenantSqlExecutor`] batch.
pub fn statements_since(
    from_version: i32,
    provider: &str,
    database: &str,
) -> Result<Vec<String>, SchemaError> {
    Ok(steps_since(from_version, provider, database)?
        .into_iter()
        .flat_map(|s| s.statements)
        .collect())
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_version_matches_the_step_list() {
        let steps = platform_steps("neon", "neondb").unwrap();
        assert_eq!(
            steps.len() as i32,
            PLATFORM_SCHEMA_VERSION,
            "adding a step without bumping PLATFORM_SCHEMA_VERSION would leave \
             tenants recorded as up to date while missing the change"
        );
    }

    #[test]
    fn step_versions_are_dense_and_ordered_from_one() {
        for (i, step) in platform_steps("neon", "neondb").unwrap().iter().enumerate() {
            assert_eq!(step.version, i as i32 + 1, "step versions must be 1..=N");
        }
    }

    #[test]
    fn a_fresh_tenant_gets_every_step() {
        let all = platform_steps("neon", "neondb").unwrap();
        let fresh = steps_since(0, "neon", "neondb").unwrap();
        assert_eq!(fresh, all);
    }

    #[test]
    fn an_up_to_date_tenant_gets_nothing() {
        let none = steps_since(PLATFORM_SCHEMA_VERSION, "neon", "neondb").unwrap();
        assert!(none.is_empty(), "got {none:#?}");
    }

    #[test]
    fn every_statement_is_idempotent_shaped() {
        // Reconcile re-runs steps to repair drift, so a bare CREATE would fail
        // the second time and wedge the tenant.
        for step in platform_steps("neon", "neondb").unwrap() {
            for stmt in &step.statements {
                let creates_object = stmt.starts_with("CREATE SCHEMA")
                    || stmt.starts_with("CREATE TABLE")
                    || stmt.starts_with("CREATE INDEX");
                if creates_object {
                    assert!(
                        stmt.contains("IF NOT EXISTS"),
                        "non-idempotent statement: {stmt}"
                    );
                }
                if stmt.contains("CREATE ROLE") {
                    assert!(
                        stmt.contains("IF NOT EXISTS") || stmt.contains("pg_roles"),
                        "CREATE ROLE has no IF NOT EXISTS in Postgres; guard it: {stmt}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_analyst_cannot_read_oxys_own_bookkeeping() {
        let sql = statements_since(0, "neon", "neondb").unwrap().join("; ");
        assert!(
            sql.contains(&format!(
                r#"REVOKE ALL ON SCHEMA "{META_SCHEMA}" FROM "{}""#,
                analyst_role_for("neon", "neondb")
            )),
            "got {sql}"
        );
    }

    #[test]
    fn the_inbox_is_oxys_and_hidden_from_the_analyst() {
        let sql = statements_since(3, "neon", "neondb").unwrap().join("; ");
        assert!(
            sql.contains(&format!(r#"CREATE SCHEMA IF NOT EXISTS "{INBOX_SCHEMA}""#)),
            "got {sql}"
        );
        assert!(
            sql.contains(&format!(
                r#"REVOKE ALL ON SCHEMA "{INBOX_SCHEMA}" FROM "{}""#,
                analyst_role_for("neon", "neondb")
            )),
            "an inbox is addressed to people; analytics must not see it by default: {sql}"
        );
        assert_eq!(
            INBOX_MESSAGES_TABLE,
            format!("{INBOX_SCHEMA}.messages"),
            "the table the reader queries is the one the step creates"
        );
    }

    #[test]
    fn the_outbox_is_written_only_through_the_definer_function() {
        // A shared-cluster provider, so the analyst revoked is the qualified
        // one that serves queries, not the bare decoy. Step 5 alone: step 6
        // replaces its function (`platform::outbox`).
        let sql = steps_since(4, "local", "oxy_org_x")
            .unwrap()
            .into_iter()
            .filter(|step| step.version == 5)
            .flat_map(|step| step.statements)
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            sql.contains("CREATE SCHEMA IF NOT EXISTS oxy_events"),
            "got {sql}"
        );
        assert!(
            sql.contains("SECURITY DEFINER SET search_path = pg_catalog"),
            "the definer must pin its search_path: {sql}"
        );
        assert!(
            sql.contains("VALUES (p_id, session_user,"),
            "the row records the calling role, never a parameter: {sql}"
        );
        // A writer can call the function with any arguments, from its own
        // SQL as well as through the host: the bounds every honest publish is
        // held to are the function's too, checked before the insert.
        for bound in [
            "IF p_depth IS NULL OR p_depth < 0 OR p_depth > 8 THEN",
            "IF p_delay_seconds IS NULL OR p_delay_seconds < 0 OR p_delay_seconds > 86400 THEN",
            "IF p_key IS NULL OR char_length(p_key) < 1 OR char_length(p_key) > 200 THEN",
            "IF p_data IS NULL OR octet_length(p_data::text) > 65536 THEN",
        ] {
            assert!(sql.contains(bound), "missing `{bound}`: {sql}");
        }
        assert!(
            sql.contains("RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER"),
            "the bounds raise, so the body is PL/pgSQL: {sql}"
        );
        assert_eq!(
            sql.matches("USING ERRCODE = 'invalid_parameter_value'")
                .count(),
            4,
            "each bound refuses with one SQLSTATE: {sql}"
        );
        assert!(
            sql.contains("PRIMARY KEY (writer_role, id))"),
            "a writer's ids are its own: {sql}"
        );
        assert!(
            sql.contains("ON CONFLICT (writer_role, id) DO NOTHING"),
            "a repeat is a no-op only within the writer's own ids: {sql}"
        );
        assert!(!sql.contains("id uuid PRIMARY KEY"), "{sql}");
        // A relayed row stays as the body's log; the claim reads waiting rows
        // only, through an index that holds nothing else.
        assert!(sql.contains("relayed_at timestamptz,"), "{sql}");
        assert!(
            sql.contains(
                "CREATE INDEX IF NOT EXISTS outbox_unrelayed ON oxy_events.outbox (created_at) \
                 WHERE relayed_at IS NULL"
            ),
            "{sql}"
        );
        assert!(
            !sql.contains("outbox_created_at"),
            "no index over relayed rows: {sql}"
        );
        assert!(
            sql.contains(
                "REVOKE ALL ON FUNCTION oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,\
                 uuid,uuid,int,uuid,int) FROM PUBLIC"
            ),
            "Postgres grants EXECUTE to PUBLIC by default: {sql}"
        );
        assert!(
            sql.contains(&format!(
                r#"REVOKE ALL ON SCHEMA oxy_events FROM "{}""#,
                analyst_role_for("local", "oxy_org_x")
            )),
            "got {sql}"
        );
    }

    #[test]
    fn the_migration_ledger_is_keyed_per_app() {
        let sql = statements_since(0, "neon", "neondb").unwrap().join("; ");
        assert!(sql.contains("PRIMARY KEY (app_id, version)"), "got {sql}");
        assert!(
            sql.contains("checksum"),
            "editing a shipped migration must be detectable"
        );
    }

    #[test]
    fn an_invalid_database_name_is_rejected_rather_than_escaped() {
        assert!(platform_steps("neon", "neon\"db").is_err());
    }
}
