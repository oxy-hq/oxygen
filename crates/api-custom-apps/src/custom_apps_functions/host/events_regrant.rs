//! When a publish grants again (app events design §6.2): a grant this process
//! remembers that the store no longer holds, or a store a reset wound back to
//! an older platform version, where the definer the host calls is missing.
//! What `events_op`'s two publish paths read and retry on.

use std::sync::LazyLock;

use oxy_oltp::platform::EVENTS_PUBLISH_FUNCTION;

/// Whether the session's role may publish through the current definer,
/// [`EVENTS_PUBLISH_FUNCTION`] by its exact signature: `USAGE` on its schema
/// and `EXECUTE` on it, read from the catalogs so the read itself never fails
/// (naming the function would without `USAGE`, `to_regprocedure` included).
/// `false` when the store has no such function — one reset to a copy of an
/// older platform version, whose only `publish` takes other arguments.
pub(super) static MAY_PUBLISH_SQL: LazyLock<String> =
    LazyLock::new(|| may_publish_sql(EVENTS_PUBLISH_FUNCTION));

/// A read that locks a catalog no app statement touches, which makes the
/// handle's session take in the catalog changes other sessions committed since
/// its transaction began — the grant just made again among them. Without it
/// the publish on the same transaction still sees the privilege as it was when
/// the transaction first looked, and is refused.
pub(super) const SEE_NEW_GRANTS: &str = "SELECT 1 FROM pg_catalog.pg_seclabel LIMIT 0";

/// [`MAY_PUBLISH_SQL`] for `signature`, `schema.name(type,…)` as `GRANT`
/// names a function. The argument types are matched by OID, each resolved
/// from its name as Postgres resolves the signature (`int` is `integer`),
/// against `proargtypes`, an array numbered from 0.
fn may_publish_sql(signature: &str) -> String {
    let (name, args) = signature
        .strip_suffix(')')
        .and_then(|signature| signature.split_once('('))
        .expect("the definer's signature is name(type,…)");
    let (schema, function) = name
        .split_once('.')
        .expect("the definer's name is schema-qualified");
    let last = args.split(',').count() - 1;
    format!(
        "SELECT coalesce(bool_and(\
         has_schema_privilege(n.oid, 'USAGE') AND has_function_privilege(p.oid, 'EXECUTE')\
         ), false) AS may_publish \
         FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = '{schema}' AND p.proname = '{function}' \
         AND p.proargtypes::pg_catalog.oid[] = \
         '[0:{last}]={{{args}}}'::pg_catalog.regtype[]::pg_catalog.oid[]"
    )
}

/// Whether a publish failed because the store lost what this process
/// remembers granting there, as the connector writes it (`query failed:
/// [<SQLSTATE>] …`): the privilege (`42501`, the role re-minted), the current
/// definer (`42883`, a reset copied a store from before it), or the events
/// schema itself (`3F000`, a copy from before the outbox). Each is mended by
/// granting again, which brings the store current first.
pub(super) fn store_lost_the_definer_or_grant(error: &str) -> bool {
    ["[42501]", "[42883]", "[3F000]"]
        .iter()
        .any(|code| error.contains(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lost_grant_definer_or_schema_is_granted_again() {
        for lost in [
            "query failed: [42501] permission denied for function publish\nSQL: SELECT …",
            "query failed: [42883] function oxy_events.publish(uuid, text, …, text) does not \
             exist\nSQL: SELECT …",
            "query failed: [3F000] schema \"oxy_events\" does not exist\nSQL: SELECT …",
        ] {
            assert!(store_lost_the_definer_or_grant(lost), "{lost}");
        }
        for other in [
            "query failed: [23505] duplicate key value violates unique constraint",
            "query failed: [22023] oxy_events.publish: depth 9 is outside 0 to 8",
            "connection error: closed",
        ] {
            assert!(!store_lost_the_definer_or_grant(other), "{other}");
        }
    }

    /// The probe is built from the constant the grant names, so the two
    /// cannot drift: it matches the current definer's thirteen arguments, by
    /// OID, and nothing of the version-5 one's twelve.
    #[test]
    fn the_probe_matches_the_current_definer_by_its_whole_signature() {
        let sql = MAY_PUBLISH_SQL.as_str();
        assert!(
            sql.contains("n.nspname = 'oxy_events' AND p.proname = 'publish'"),
            "{sql}"
        );
        assert!(
            sql.contains(
                "'[0:12]={uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int,bytea}'\
                 ::pg_catalog.regtype[]"
            ),
            "{sql}"
        );
        assert!(
            !sql.contains("to_regprocedure"),
            "fails without USAGE: {sql}"
        );
        let older = may_publish_sql(
            "oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int)",
        );
        assert!(
            older.contains("'[0:11]={uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int}'"),
            "{older}"
        );
        assert_ne!(older, *sql);
    }
}
