//! The events outbox's steps after the one that created it (platform step 5,
//! in [`super::platform_steps`]).

use super::{
    EVENT_MAX_DATA_TEXT_BYTES, EVENT_MAX_DELAY_SECONDS, EVENT_MAX_DEPTH, EVENT_MAX_KEY_CHARS,
    EVENT_PROVENANCE_BYTES, EVENTS_OUTBOX_TABLE, EVENTS_PUBLISH_FUNCTION,
    EVENTS_PUBLISH_FUNCTION_V5, EVENTS_SCHEMA, PlatformStep,
};

mod invoker;
pub(super) use invoker::invoker_step;

/// Step 6: the host's provenance MAC on every outbox row (app events design
/// §6.2, ruling R25(c)).
///
/// App SQL can call the definer itself, so the host signs every row it
/// writes (`HMAC-SHA256` under a key of the org's own, which never enters
/// this database) and the relay delivers only a row whose MAC matches its
/// fields. The definer cannot check a MAC; it stores it beside the row, as
/// the last argument of a new signature, and holds it to 32 bytes so the
/// column holds nothing else.
///
/// **The unsigned signature is dropped**, not kept callable: a server still
/// running the code from before this step then fails its publish loudly
/// (`undefined_function`) instead of writing a row nothing signed, which the
/// relay would quarantine without the app ever hearing of it. The drop takes
/// the old function's grants with it, so the grant audit sees only the new
/// one. `CASCADE`: an object of an app's own that names the old function
/// (only a view records that dependency) would otherwise wedge this step,
/// and with it the org's every grant and claim; it could only ever have
/// published rows nothing signed.
///
/// The new column is nullable: a row written before this step has none, and
/// the relay quarantines it like any row without a valid MAC.
pub(super) fn provenance_step() -> PlatformStep {
    PlatformStep {
        version: 6,
        description: "events outbox provenance MAC, through a new definer function",
        statements: vec![
            format!("ALTER TABLE {EVENTS_OUTBOX_TABLE} ADD COLUMN IF NOT EXISTS provenance bytea"),
            format!("DROP FUNCTION IF EXISTS {EVENTS_PUBLISH_FUNCTION_V5} CASCADE"),
            signed_publish_function(),
            format!("REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION} FROM PUBLIC"),
        ],
    }
}

/// Step 5's function with the MAC: the same bounds, checked before the
/// insert, and the MAC's, then the row with the MAC beside it.
fn signed_publish_function() -> String {
    format!(
        "CREATE OR REPLACE FUNCTION {EVENTS_SCHEMA}.publish(
  p_id uuid, p_environment text, p_name text, p_key text, p_data jsonb, p_schema_revision uuid,
  p_actor text, p_causation_id uuid, p_root_id uuid, p_depth int, p_invocation_id uuid, p_delay_seconds int,
  p_provenance bytea)
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
  IF p_provenance IS NULL OR octet_length(p_provenance) <> {EVENT_PROVENANCE_BYTES} THEN
    RAISE EXCEPTION 'oxy_events.publish: provenance is % bytes; it must be {EVENT_PROVENANCE_BYTES}', octet_length(p_provenance)
      USING ERRCODE = 'invalid_parameter_value';
  END IF;
  INSERT INTO {EVENTS_OUTBOX_TABLE} (id, writer_role, environment, name, key, data, schema_revision,
    actor, causation_id, root_id, depth, invocation_id, delay_seconds, provenance)
  VALUES (p_id, session_user, p_environment, p_name, p_key, p_data, p_schema_revision,
    p_actor, p_causation_id, p_root_id, p_depth, p_invocation_id, p_delay_seconds, p_provenance)
  ON CONFLICT (writer_role, id) DO NOTHING;
  RETURN p_id;
END
$$"
    )
}

#[cfg(test)]
mod tests {
    use super::super::{
        EVENT_PROVENANCE_BYTES, EVENTS_PUBLISH_FUNCTION, EVENTS_PUBLISH_FUNCTION_V5, platform_steps,
    };

    /// Step 6's statements alone: a later step replaces its definer again.
    fn step_six() -> String {
        let step = platform_steps("local", "oxy_org_x").unwrap().remove(5);
        assert_eq!(step.version, 6);
        step.statements.join("; ")
    }

    /// R25(c): the definer stores the MAC the host signed, beside the row it
    /// signs, and refuses one of any other length: it cannot check it (the
    /// key never enters the store), but it holds the column to what the
    /// relay reads.
    #[test]
    fn step_six_stores_the_hosts_mac_through_a_new_definer() {
        let sql = step_six();
        assert!(
            sql.contains("ALTER TABLE oxy_events.outbox ADD COLUMN IF NOT EXISTS provenance bytea"),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "CREATE OR REPLACE FUNCTION oxy_events.publish(\n  p_id uuid, p_environment text, \
                 p_name text, p_key text, p_data jsonb, p_schema_revision uuid,\n  p_actor text, \
                 p_causation_id uuid, p_root_id uuid, p_depth int, p_invocation_id uuid, \
                 p_delay_seconds int,\n  p_provenance bytea)"
            )),
            "{sql}"
        );
        assert!(
            sql.contains("SECURITY DEFINER SET search_path = pg_catalog"),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "IF p_provenance IS NULL OR octet_length(p_provenance) <> {EVENT_PROVENANCE_BYTES} \
                 THEN"
            )),
            "{sql}"
        );
        assert!(sql.contains("VALUES (p_id, session_user,"), "{sql}");
        assert!(sql.contains("p_delay_seconds, p_provenance)"), "{sql}");
        assert!(
            sql.contains("ON CONFLICT (writer_role, id) DO NOTHING"),
            "{sql}"
        );
        // Every bound step 5's function held, and the MAC's: one SQLSTATE.
        for bound in [
            "IF p_depth IS NULL OR p_depth < 0 OR p_depth > 8 THEN",
            "IF p_delay_seconds IS NULL OR p_delay_seconds < 0 OR p_delay_seconds > 86400 THEN",
            "IF p_key IS NULL OR char_length(p_key) < 1 OR char_length(p_key) > 200 THEN",
            "IF p_data IS NULL OR octet_length(p_data::text) > 65536 THEN",
        ] {
            assert!(sql.contains(bound), "missing `{bound}`: {sql}");
        }
        assert_eq!(
            sql.matches("USING ERRCODE = 'invalid_parameter_value'")
                .count(),
            5,
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                "REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION} FROM PUBLIC"
            )),
            "Postgres grants EXECUTE to PUBLIC by default: {sql}"
        );
    }

    /// A server still calling the old signature fails loudly (the function is
    /// gone) rather than writing a row nothing signed; the drop takes the
    /// old function's grants with it.
    #[test]
    fn step_six_drops_the_unsigned_signature() {
        let sql = step_six();
        assert!(
            sql.contains(&format!(
                "DROP FUNCTION IF EXISTS {EVENTS_PUBLISH_FUNCTION_V5} CASCADE"
            )),
            "{sql}"
        );
        let drop = sql.find("DROP FUNCTION").expect("the drop");
        let create = sql.find("CREATE OR REPLACE FUNCTION").expect("the create");
        assert!(drop < create, "dropped before the new one exists: {sql}");
        assert_ne!(EVENTS_PUBLISH_FUNCTION, EVENTS_PUBLISH_FUNCTION_V5);
        assert!(
            EVENTS_PUBLISH_FUNCTION.ends_with(",int,bytea)")
                && EVENTS_PUBLISH_FUNCTION
                    .starts_with(EVENTS_PUBLISH_FUNCTION_V5.trim_end_matches(')')),
            "the new signature is the old one and the MAC"
        );
    }

    /// Step 5 is shipped, so it still revokes the function it created; a
    /// fresh tenant runs it before step 6 exists.
    #[test]
    fn step_five_still_names_its_own_function() {
        let step_five = &platform_steps("neon", "neondb").unwrap()[4];
        assert_eq!(step_five.version, 5);
        assert!(
            step_five.statements.contains(&format!(
                "REVOKE ALL ON FUNCTION {EVENTS_PUBLISH_FUNCTION_V5} FROM PUBLIC"
            )),
            "{:?}",
            step_five.statements
        );
    }
}
