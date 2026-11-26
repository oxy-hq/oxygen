//! Who builds a change's image: a role with no privileges, so an app's cast
//! never runs as the store's owner (OLTP change stream design §5.2,
//! "Hardening").
//!
//! A capture function is `SECURITY DEFINER`, owned by the store's owner, and
//! an image is the row as `jsonb`. `to_jsonb` converts a value of a type that
//! is not built in through that type's cast to `json` when one exists `WITH
//! FUNCTION`, and calls the function directly, as whoever is running
//! (Postgres 18's `json_categorize_type` and `datum_to_jsonb_internal`). It
//! does so for every column, excluded ones too, through domains, array
//! elements and composite fields. An app's writer owns the types its
//! migrations create, and may create a cast from one. Called straight from a
//! capture function, that cast would run as the owner, who may write
//! `oxy_changes`.
//!
//! A catalog check before the image cannot close this: it reads `pg_cast`
//! through the statement's snapshot, while `to_jsonb` finds casts through the
//! syscache, which sees a cast another session committed later. So capture
//! never calls `to_jsonb` itself. It calls its writer's [`image_function`]
//! (`oxy_changes.image_<writer>(anyelement)`, `SELECT pg_catalog.to_jsonb($1)`),
//! a definer owned by that writer's [`image_role_for`]: a `NOLOGIN` role that
//! holds no privilege it was not given and is a member of nothing. Any cast
//! then runs as that role. It decides only how its own type's values appear
//! in the app's own history; it can neither read nor write `oxy_changes`; and
//! one that fails, fails the app's write. Only the store's owner may execute
//! the function. One role per writer, not per store: an app may grant on its
//! own objects to any role, and what it grants its own image role reaches
//! only its own casts. The cost is one definer call per image, two per row
//! (key and image), four per updated row.
//!
//! Each owner install makes sure of the role and the function first
//! ([`image_sql`]): it creates the role if absent, grants itself `SET` on it
//! (not `INHERIT`), and replaces the function as that role. The role needs
//! `USAGE` and `CREATE` on `oxy_changes` only while it does, so both are
//! granted and revoked inside the install's transaction, which no other
//! session sees. An advisory lock serializes concurrent installs on one
//! store.
//!
//! The owner's own session has a `search_path` naming every app's schema
//! (`schema::database_search_path_sql`), so an operator or function an app
//! plants there could resolve ahead of `pg_catalog`'s for an expression that
//! needs a coercion. Every statement the install runs as the owner therefore
//! runs under `pg_catalog, pg_temp`: [`OWNER_SESSION_PIN_SQL`] on the
//! install's own session, and `SET LOCAL` at the head of each owner install.

use crate::platform::{CHANGES_IMAGE_FUNCTION_PREFIX, CHANGES_IMAGE_ROLE, CHANGES_SCHEMA};
use crate::roles::quote_literal;
use crate::schema::{SchemaError, quote_ident, shares_role_namespace, tenant_tag};
use sha2::{Digest, Sha256};

/// Postgres keeps 63 bytes of an identifier and drops the rest.
const MAX_ROLE_LEN: usize = 63;
/// Hex digits of SHA-256 a cut-down image role name ends with: 96 bits.
const DIGEST_HEX: usize = 24;

/// Pins a session the install opens as the store's owner to Postgres's own
/// names, so nothing an app planted on the owner's default path resolves.
/// Not on the owner's DSN: a workspace's migrations run as the owner and use
/// that path.
pub const OWNER_SESSION_PIN_SQL: &str = "SET search_path = pg_catalog, pg_temp";

/// The same, for one transaction: the head of every owner install.
pub(super) const OWNER_TRANSACTION_PIN_SQL: &str = "SET LOCAL search_path = pg_catalog, pg_temp";

/// The image role of `writer` (its bare name): `oxy_changes_image_<writer>`
/// on a provider that gives each tenant its own cluster (Neon), and with the
/// tenant's tag before the writer, `oxy_changes_image_<tag>_<writer>`, on one
/// that does not, so [`image_role_prefix`] finds every one of a tenant's. One
/// per writer: an app may grant on its own objects to any role, and a grant to
/// its own image role must reach no other app's cast. A name past 63 bytes
/// keeps its first 38 and 96 bits of the whole's SHA-256 (24 hex), so it
/// still fits, its tenant's prefix kept, and no slug can be chosen to match
/// another writer's; the install also refuses a role recorded for another
/// writer ([`image_sql`]).
pub fn image_role_for(provider: &str, database: &str, writer: &str) -> String {
    let name = if shares_role_namespace(provider) {
        format!("{}{writer}", image_role_prefix(database))
    } else {
        format!("{CHANGES_IMAGE_ROLE}_{writer}")
    };
    if name.len() <= MAX_ROLE_LEN {
        return name;
    }
    let digest: String = Sha256::digest(name.as_bytes())
        .iter()
        .take(DIGEST_HEX / 2)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{}_{digest}", &name[..MAX_ROLE_LEN - DIGEST_HEX - 1])
}

/// What an image role's comment says it is, for `writer`: how the install
/// tells a role it made for this writer from one some other writer's name
/// cut down to the same.
fn image_role_record(writer: &str) -> String {
    format!("oxy_changes image role for {writer}")
}

/// What every image role of the tenant in `database` starts with, on a
/// provider that shares one cluster among tenants: what its teardown drops.
pub fn image_role_prefix(database: &str) -> String {
    format!("{CHANGES_IMAGE_ROLE}_{}_", tenant_tag(database))
}

/// `oxy_changes."image_<writer>"`: `writer`'s image function, which its
/// capture functions call.
pub fn image_function(writer: &str) -> String {
    format!(
        "{CHANGES_SCHEMA}.{}",
        quote_ident(&format!("{CHANGES_IMAGE_FUNCTION_PREFIX}{writer}"))
    )
}

/// A role name the image SQL may quote: lowercase letters, digits and
/// underscores, starting with a letter, and whole within 63 bytes.
fn validate_role(name: &str) -> Result<(), SchemaError> {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if starts && rest && name.len() <= MAX_ROLE_LEN {
        Ok(())
    } else {
        Err(SchemaError::InvalidName(name.to_string()))
    }
}

/// The owner's statements making sure `image_role` exists with no
/// privileges and owns `writer`'s [`image_function`], which only the store's
/// owner may execute. Idempotent; for one transaction, after
/// [`OWNER_TRANSACTION_PIN_SQL`]. `SET ROLE` rather than `SET LOCAL ROLE`:
/// outside a transaction `SET LOCAL` does nothing, and the function would be
/// the owner's. A role it creates is recorded as `writer`'s (its comment); one
/// that exists recorded for anything else refuses the install, naming both.
pub(super) fn image_sql(writer: &str, image_role: &str) -> Result<Vec<String>, SchemaError> {
    validate_role(image_role)?;
    let role = quote_ident(image_role);
    let name = quote_literal(image_role);
    let record = quote_literal(&image_role_record(writer));
    let function = format!("{}(anyelement)", image_function(writer));
    Ok(vec![
        // One lock for every writer's: each install grants on `oxy_changes`.
        "SELECT pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtext('oxy_changes.image'))"
            .to_string(),
        format!(
            "DO $oxy$ DECLARE v_record text; BEGIN \
             IF NOT EXISTS (SELECT FROM pg_catalog.pg_roles WHERE rolname = {name}) THEN \
               CREATE ROLE {role} NOLOGIN; COMMENT ON ROLE {role} IS {record}; \
             ELSE \
               SELECT pg_catalog.shobj_description(r.oid, 'pg_authid') INTO v_record \
                 FROM pg_catalog.pg_roles r WHERE r.rolname = {name}; \
               IF v_record IS DISTINCT FROM {record} THEN \
                 RAISE EXCEPTION 'oxy_changes: writer % cannot use image role %, which is \
                   recorded as %', {writer}, {name}, coalesce(v_record, 'nothing') \
                   USING ERRCODE = 'duplicate_object'; \
               END IF; \
             END IF; END $oxy$",
            writer = quote_literal(writer),
        ),
        format!("GRANT {role} TO CURRENT_USER WITH INHERIT FALSE, SET TRUE"),
        format!("GRANT USAGE, CREATE ON SCHEMA {CHANGES_SCHEMA} TO {role}"),
        format!("SET ROLE {role}"),
        format!(
            "CREATE OR REPLACE FUNCTION {function} RETURNS jsonb LANGUAGE sql STABLE \
             SECURITY DEFINER SET search_path = pg_catalog, pg_temp \
             AS $oxy$ SELECT pg_catalog.to_jsonb($1) $oxy$"
        ),
        format!("REVOKE ALL ON FUNCTION {function} FROM PUBLIC"),
        format!("GRANT EXECUTE ON FUNCTION {function} TO SESSION_USER"),
        "RESET ROLE".to_string(),
        format!("REVOKE USAGE, CREATE ON SCHEMA {CHANGES_SCHEMA} FROM {role}"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql() -> Vec<String> {
        image_sql("x", "oxy_changes_image_x").expect("a valid role name")
    }

    fn position(sql: &[String], prefix: &str) -> usize {
        sql.iter()
            .position(|s| s.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix}: {sql:#?}"))
    }

    #[test]
    fn each_writer_has_its_own_role_and_function() {
        assert_eq!(
            image_role_for("neon", "neondb", "bookings"),
            "oxy_changes_image_bookings"
        );
        let local = image_role_for("local", "oxy_org_x", "bookings");
        let prefix = image_role_prefix("oxy_org_x");
        assert_eq!(local, format!("{prefix}bookings"));
        assert_ne!(local, image_role_for("local", "oxy_org_x", "notes"));
        assert_ne!(prefix, image_role_prefix("oxy_org_y"));
        assert_eq!(image_function("bookings"), "oxy_changes.\"image_bookings\"");
        assert!(image_sql("x", "bad role").is_err());
    }

    /// The longest writer still gets a whole name, its tenant's prefix kept,
    /// and two that share the cut-down prefix differ in 96 bits of SHA-256.
    #[test]
    fn a_long_name_fits_and_stays_distinct() {
        let long = "w".repeat(55);
        for provider in ["local", "neon"] {
            let a = image_role_for(provider, "oxy_org_x", &format!("{long}a"));
            let b = image_role_for(provider, "oxy_org_x", &format!("{long}b"));
            assert_eq!(a.len(), MAX_ROLE_LEN, "{a}");
            assert_eq!(
                a[..MAX_ROLE_LEN - DIGEST_HEX],
                b[..MAX_ROLE_LEN - DIGEST_HEX]
            );
            assert_ne!(a, b);
            let digest = &a[MAX_ROLE_LEN - DIGEST_HEX..];
            assert!(digest.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
            assert!(validate_role(&a).is_ok(), "{a}");
        }
        let local = image_role_for("local", "oxy_org_x", &format!("{long}a"));
        assert!(
            local.starts_with(&image_role_prefix("oxy_org_x")),
            "{local}"
        );
    }

    /// A role the install creates is recorded as its writer's, and one that
    /// exists recorded otherwise refuses the install, naming both.
    #[test]
    fn a_role_recorded_for_another_writer_is_refused() {
        let check = &sql()[1];
        assert!(
            check.contains(
                "CREATE ROLE \"oxy_changes_image_x\" NOLOGIN; \
                 COMMENT ON ROLE \"oxy_changes_image_x\" IS 'oxy_changes image role for x';"
            ),
            "{check}"
        );
        assert!(
            check.contains("IF v_record IS DISTINCT FROM 'oxy_changes image role for x' THEN"),
            "{check}"
        );
        assert!(
            check.contains("USING ERRCODE = 'duplicate_object'"),
            "{check}"
        );
    }

    /// The function is created and its grants set as the image role, so it
    /// owns it; the role holds the schema's `USAGE` and `CREATE` only around
    /// that, and the owner may only switch to it, not inherit it.
    #[test]
    fn the_function_is_the_image_roles_and_the_schema_is_not() {
        let sql = sql();
        assert!(sql[0].starts_with("SELECT pg_catalog.pg_advisory_xact_lock("));
        assert!(
            sql[1].contains("THEN CREATE ROLE \"oxy_changes_image_x\" NOLOGIN;"),
            "{}",
            sql[1]
        );
        assert_eq!(
            sql[2],
            "GRANT \"oxy_changes_image_x\" TO CURRENT_USER WITH INHERIT FALSE, SET TRUE"
        );
        let granted = position(&sql, "GRANT USAGE, CREATE ON SCHEMA oxy_changes");
        let set = position(&sql, "SET ROLE \"oxy_changes_image_x\"");
        let create = position(
            &sql,
            "CREATE OR REPLACE FUNCTION oxy_changes.\"image_x\"(anyelement)",
        );
        let revoke = position(
            &sql,
            "REVOKE ALL ON FUNCTION oxy_changes.\"image_x\"(anyelement)",
        );
        let execute = position(
            &sql,
            "GRANT EXECUTE ON FUNCTION oxy_changes.\"image_x\"(anyelement)",
        );
        let reset = position(&sql, "RESET ROLE");
        let revoked = position(&sql, "REVOKE USAGE, CREATE ON SCHEMA oxy_changes");
        assert!(
            granted < set
                && set < create
                && create < revoke
                && revoke < execute
                && execute < reset
                && reset < revoked
                && revoked == sql.len() - 1,
            "{sql:#?}"
        );
        assert_eq!(
            sql[execute],
            "GRANT EXECUTE ON FUNCTION oxy_changes.\"image_x\"(anyelement) TO SESSION_USER"
        );
    }

    #[test]
    fn the_function_is_a_pinned_definer_around_to_jsonb() {
        let sql = sql();
        let create = &sql[position(&sql, "CREATE OR REPLACE FUNCTION")];
        assert!(
            create.contains(
                "RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER \
                 SET search_path = pg_catalog, pg_temp"
            ),
            "{create}"
        );
        assert!(
            create.ends_with("AS $oxy$ SELECT pg_catalog.to_jsonb($1) $oxy$"),
            "{create}"
        );
    }
}
