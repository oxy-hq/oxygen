//! Step 7: who a `ctx.apps.send` ran as, signed beside the row (app events
//! design §8, custom apps architecture §6.1).
//!
//! A send is delivered as the person who sent it only when the host says the
//! sending run ran directly as that person, and under which credential (a
//! login session, or a stored token whose ceiling a delivery re-reads). The
//! host signs those facts with a second MAC (`app_events::provenance`,
//! message domain v2) and the definer stores all three beside the row:
//! `invoker` (`session` or `token`), `invoker_token_id`, and
//! `invoker_provenance`, the 32-byte MAC.
//!
//! **The definer keeps its signature.** Step 6 dropped the signature before it,
//! so a server one release back failed every publish. This step must not: a
//! previous binary has to keep publishing against this schema, and its grant
//! audit refuses a writer holding `EXECUTE` on any `oxy_events` function but
//! the one it knows, so a second, wider function would wedge every writer's
//! credentials on a revert. So the function is replaced in place, the same
//! thirteen arguments and the same grants, and reads the three facts from
//! transaction-local settings the host sets in the publish statement itself
//! (`oxy_events::PUBLISH_WITH_INVOKER_SQL`). It reads each once and clears it,
//! so nothing later in the transaction inherits it.
//!
//! **Nothing here is trusted.** App SQL can set the same settings and call the
//! definer, as it can call the definer with any argument; what it cannot do is
//! sign. The relay delivers a row as a person only when the invoker MAC is the
//! host's over the row and these three columns. A value that is not
//! well-formed (a kind that is neither, a token id that is not a UUID, a MAC
//! that is not 64 hex characters) is stored as no invoker at all, never
//! raised: a publish with a stray setting is still a publish, and a row
//! without an invoker is a system delivery, as every send was before.
//!
//! The columns are nullable: a row written before this step, or by a server
//! that sets none of the settings, has no invoker, and its send runs as the
//! system.

use crate::platform::{
    EVENT_INVOKER_PROVENANCE_SETTING, EVENT_INVOKER_SETTING, EVENT_INVOKER_TOKEN_SETTING,
    EVENT_MAX_DATA_TEXT_BYTES, EVENT_MAX_DELAY_SECONDS, EVENT_MAX_DEPTH, EVENT_MAX_KEY_CHARS,
    EVENT_PROVENANCE_BYTES, EVENTS_OUTBOX_TABLE, EVENTS_PUBLISH_FUNCTION, EVENTS_SCHEMA,
    PlatformStep,
};

/// Step 7, as the module docs describe it.
pub(in crate::platform) fn invoker_step() -> PlatformStep {
    PlatformStep {
        version: 7,
        description: "events outbox send invoker, signed, through the same definer",
        statements: vec![
            format!("ALTER TABLE {EVENTS_OUTBOX_TABLE} ADD COLUMN IF NOT EXISTS invoker text"),
            format!(
                "ALTER TABLE {EVENTS_OUTBOX_TABLE} ADD COLUMN IF NOT EXISTS invoker_token_id uuid"
            ),
            format!(
                "ALTER TABLE {EVENTS_OUTBOX_TABLE} ADD COLUMN IF NOT EXISTS invoker_provenance bytea"
            ),
            invoker_publish_function(),
            format!("REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION} FROM PUBLIC"),
        ],
    }
}

/// Step 6's function, its signature unchanged, storing the invoker the
/// publish statement set. The bounds and their order are step 6's.
fn invoker_publish_function() -> String {
    format!(
        "CREATE OR REPLACE FUNCTION {EVENTS_SCHEMA}.publish(
  p_id uuid, p_environment text, p_name text, p_key text, p_data jsonb, p_schema_revision uuid,
  p_actor text, p_causation_id uuid, p_root_id uuid, p_depth int, p_invocation_id uuid, p_delay_seconds int,
  p_provenance bytea)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
  v_invoker text := NULLIF(current_setting('{EVENT_INVOKER_SETTING}', true), '');
  v_token text := NULLIF(current_setting('{EVENT_INVOKER_TOKEN_SETTING}', true), '');
  v_mac text := NULLIF(current_setting('{EVENT_INVOKER_PROVENANCE_SETTING}', true), '');
BEGIN
  PERFORM set_config('{EVENT_INVOKER_SETTING}', '', true);
  PERFORM set_config('{EVENT_INVOKER_TOKEN_SETTING}', '', true);
  PERFORM set_config('{EVENT_INVOKER_PROVENANCE_SETTING}', '', true);
  IF v_invoker IS NULL OR v_mac IS NULL
     OR v_mac !~ '^[0-9a-f]{{64}}$'
     OR NOT ((v_invoker = 'session' AND v_token IS NULL)
          OR (v_invoker = 'token' AND v_token IS NOT NULL
              AND v_token ~ '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$')) THEN
    v_invoker := NULL;
    v_token := NULL;
    v_mac := NULL;
  END IF;
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
  IF p_provenance IS NULL OR octet_length(p_provenance) <> {EVENT_PROVENANCE_BYTES} THEN
    RAISE EXCEPTION 'oxy_events.publish: provenance is % bytes; it must be {EVENT_PROVENANCE_BYTES}', octet_length(p_provenance)
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  INSERT INTO {EVENTS_OUTBOX_TABLE} (id, writer_role, environment, name, key, data, schema_revision,
    actor, causation_id, root_id, depth, invocation_id, delay_seconds, provenance,
    invoker, invoker_token_id, invoker_provenance)
  VALUES (p_id, session_user, p_environment, p_name, p_key, p_data, p_schema_revision,
    p_actor, p_causation_id, p_root_id, p_depth, p_invocation_id, p_delay_seconds, p_provenance,
    v_invoker, v_token::uuid, decode(v_mac, 'hex'))
  ON CONFLICT (writer_role, id) DO NOTHING;
  RETURN p_id;
END
$$"
    )
}

#[cfg(test)]
mod tests {
    use crate::platform::{
        EVENT_INVOKER_PROVENANCE_SETTING, EVENT_INVOKER_SETTING, EVENT_INVOKER_TOKEN_SETTING,
        EVENTS_PUBLISH_FUNCTION, PLATFORM_SCHEMA_VERSION, platform_steps, statements_since,
    };

    fn step_seven() -> String {
        statements_since(6, "local", "oxy_org_x")
            .unwrap()
            .join("; ")
    }

    #[test]
    fn step_seven_is_the_last_step() {
        let steps = platform_steps("neon", "neondb").unwrap();
        assert_eq!(steps[6].version, 7);
        assert_eq!(PLATFORM_SCHEMA_VERSION, 7);
    }

    /// Revert safety: a binary one release back calls the thirteen-argument
    /// definer and its grant audit knows that one function. So the step
    /// replaces it in place and creates no other, and drops nothing.
    #[test]
    fn step_seven_keeps_the_definers_signature_and_grants() {
        let sql = step_seven();
        assert!(!sql.contains("DROP FUNCTION"), "{sql}");
        assert_eq!(
            sql.matches("CREATE OR REPLACE FUNCTION").count(),
            1,
            "{sql}"
        );
        assert!(
            sql.contains(
                "CREATE OR REPLACE FUNCTION oxy_events.publish(\n  p_id uuid, p_environment text, \
                 p_name text, p_key text, p_data jsonb, p_schema_revision uuid,\n  p_actor text, \
                 p_causation_id uuid, p_root_id uuid, p_depth int, p_invocation_id uuid, \
                 p_delay_seconds int,\n  p_provenance bytea)"
            ),
            "the same thirteen arguments: {sql}"
        );
        assert!(
            EVENTS_PUBLISH_FUNCTION.ends_with(",int,bytea)"),
            "the signature the grants and the audit name is step 6's"
        );
        assert!(
            sql.contains("SECURITY DEFINER SET search_path = pg_catalog"),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION} FROM PUBLIC"
            )),
            "{sql}"
        );
    }

    /// The three columns are nullable additions, so a previous binary's
    /// insert (which names none of them) and its claim (which reads none)
    /// both keep working.
    #[test]
    fn step_seven_adds_three_nullable_columns() {
        let sql = step_seven();
        for column in [
            "invoker text",
            "invoker_token_id uuid",
            "invoker_provenance bytea",
        ] {
            assert!(
                sql.contains(&format!(
                    "ALTER TABLE oxy_events.outbox ADD COLUMN IF NOT EXISTS {column}"
                )),
                "{column}: {sql}"
            );
        }
        let step = platform_steps("local", "oxy_org_x").unwrap().remove(6);
        let alters: Vec<&String> = step
            .statements
            .iter()
            .filter(|s| s.starts_with("ALTER TABLE"))
            .collect();
        assert_eq!(alters.len(), 3, "{alters:?}");
        for alter in alters {
            assert!(
                !alter.contains("NOT NULL") && !alter.contains("DEFAULT"),
                "a nullable addition, no default: {alter}"
            );
        }
    }

    /// The definer reads each setting once, clears it before anything else,
    /// and stores what it read beside the row it signs; a malformed value is
    /// stored as no invoker, never raised.
    #[test]
    fn the_definer_stores_the_invoker_the_statement_set_and_clears_it() {
        let sql = step_seven();
        for setting in [
            EVENT_INVOKER_SETTING,
            EVENT_INVOKER_TOKEN_SETTING,
            EVENT_INVOKER_PROVENANCE_SETTING,
        ] {
            assert!(
                sql.contains(&format!("NULLIF(current_setting('{setting}', true), '')")),
                "{setting}: {sql}"
            );
            let clear = format!("PERFORM set_config('{setting}', '', true)");
            let cleared = sql.find(&clear).expect("cleared");
            assert!(
                cleared < sql.find("IF p_depth").expect("the bounds"),
                "cleared before any bound can raise: {sql}"
            );
        }
        assert!(
            sql.contains("v_invoker, v_token::uuid, decode(v_mac, 'hex'))"),
            "{sql}"
        );
        assert!(sql.contains("v_mac !~ '^[0-9a-f]{64}$'"), "{sql}");
        assert_eq!(
            sql.matches("USING ERRCODE = 'invalid_parameter_value'")
                .count(),
            5,
            "the invoker adds no refusal: {sql}"
        );
    }
}
