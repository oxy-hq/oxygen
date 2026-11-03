//! Role creation in **SQL**, deliberately not through the provider API.
//!
//! # Why not the provider's `create_role`
//!
//! Every role Neon creates through its REST API is made a member of
//! `neon_superuser` and given `CREATEDB`, `CREATEROLE` and `BYPASSRLS`. That is
//! reasonable for a project's own owner and catastrophic for anything else: it
//! makes the "read-only analyst" able to read every schema — including `app_*`
//! that never opted into analytics, and `oxy_meta` — and lets one app's writer
//! read every other app's data. The entire isolation model of this crate is
//! those two boundaries.
//!
//! It cannot be undone after the fact. The database owner is not a member with
//! ADMIN option, so `REVOKE neon_superuser FROM …` fails:
//!
//! ```text
//! ERROR: permission denied to revoke role "neon_superuser"
//! DETAIL: Only roles with the ADMIN option on role "neon_superuser" may revoke this role.
//! ```
//!
//! A role created by `CREATE ROLE` as the owner has none of it — verified
//! against live Neon: no memberships, and `createdb/createrole/bypassrls/
//! replication` all false. So Oxy mints its own roles and leaves the provider
//! API responsible for the project alone.
//!
//! What the API grants instead is not folklore. Neon's own open-source
//! `compute_ctl` spells it out in `compute_tools/src/spec_apply.rs`:
//!
//! ```text
//! CREATE ROLE x INHERIT CREATEROLE CREATEDB BYPASSRLS REPLICATION IN ROLE neon_superuser
//! ```
//!
//! [`assert_confined_sql`] checks all of those, plus `rolsuper` — Oxy's own
//! addition, since a provider that ever handed back a true superuser would
//! otherwise pass. Four attributes from Neon, five in
//! [`CONFINEMENT_ATTRIBUTES`], and a membership.
//!
//! Which is also the simpler story: `LocalProvider` already created roles in
//! SQL, so this is one path instead of two, and it works on any Postgres.
//!
//! # Least privilege, by asking for nothing
//!
//! This section used to say every attribute is spelled out rather than left to
//! the server default. The code has argued the opposite for a while and the
//! header did not follow: `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION`
//! and `BYPASSRLS` can each only be changed by a role that already holds it —
//! *even to switch it off* — so naming them turns a correct role into a failed
//! provision, and which ones fail depends on the owner. They are all off by
//! default, so [`ensure_login_role_sql`] asks for nothing and
//! [`assert_confined_sql`] verifies.
//!
//! `NOINHERIT` is the one exception and is set explicitly: not a privilege
//! attribute, so a `CREATEROLE` owner may set it, and a role that inherits
//! nothing cannot pick up privileges from a group it is later added to.
//!
//! # Fail closed
//!
//! [`assert_confined_sql`] re-reads the role after creating it and raises if it
//! came out with any authority it should not have. If a provider changes what
//! `CREATE ROLE` produces, provisioning must break loudly rather than quietly
//! hand out a superuser — this crate's whole reason for existing is a boundary,
//! and a boundary that fails open is worse than none.

use crate::schema::{SchemaError, validate_name};

/// Password length in bytes of entropy before encoding.
///
/// Neon's control plane rejects weak passwords on `CREATE ROLE` ("insecure
/// password, try including more special characters…"), so this generates from a
/// mixed alphabet rather than hex.
const PASSWORD_BYTES: usize = 24;

/// Alphabet with upper, lower, digits and symbols — enough to satisfy a
/// provider password policy without needing to know its exact rule.
const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!#%&*+-=?@";

/// A fresh random password.
///
/// Uses `getrandom` through `uuid`'s v4 generator as the entropy source, which
/// this crate already depends on, rather than adding an RNG crate for one call.
pub fn generate_password() -> String {
    // A provider policy can reject a password that happened to draw no symbol,
    // so require one of each class — by REDRAWING rather than by overwriting
    // the first four characters with `Aa7#`. Stamping a fixed prefix made every
    // password this crate mints start with the same four bytes: four of the 24
    // characters carrying no entropy, and a literal string to grep for. Missing
    // a class is rare enough (each spans a large share of a 66-character
    // alphabet, over 24 draws) that this effectively never draws twice.
    loop {
        let mut out = String::with_capacity(PASSWORD_BYTES);
        while out.len() < PASSWORD_BYTES {
            for byte in uuid::Uuid::new_v4().as_bytes() {
                if out.len() == PASSWORD_BYTES {
                    break;
                }
                out.push(ALPHABET[*byte as usize % ALPHABET.len()] as char);
            }
        }
        if has_every_class(&out) {
            return out;
        }
    }
}

/// Upper, lower, digit and symbol all present — the union of the password
/// policies this has actually hit.
fn has_every_class(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_uppercase())
        && s.chars().any(|c| c.is_ascii_lowercase())
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| !c.is_ascii_alphanumeric())
}

/// Quote a literal for SQL. Passwords come from [`generate_password`] so they
/// contain no quotes, but this is the one place a generated value reaches SQL
/// text and `CREATE ROLE` has no parameter binding.
pub(crate) fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Create `role` with a login and **no** authority beyond it.
///
/// Idempotent: an existing role has its password reset instead, which is what
/// makes re-provisioning safe and is the only way back to a usable credential
/// once the original is lost (Postgres stores only a hash).
pub fn ensure_login_role_sql(role: &str, password: &str) -> Result<Vec<String>, SchemaError> {
    validate_name(role)?;
    let pw = quote_literal(password);
    // Set NOTHING that Postgres gates on holding the attribute yourself.
    //
    // `SUPERUSER`, `CREATEDB`, `CREATEROLE`, `REPLICATION` and `BYPASSRLS` can
    // each only be changed by a role that has it — *even to switch it off*
    // ("Only roles with the CREATEDB attribute may change the CREATEDB
    // attribute"). Naming them turns a correct role into a failed provision,
    // and which ones fail depends on the owner: on Neon the owner carries them
    // through `neon_superuser` and it worked, on a plain local cluster it did
    // not. Both were caught by running it.
    //
    // They are all OFF by default on `CREATE ROLE`, so the safe construction is
    // to ask for nothing and *verify* — which is [`assert_confined_sql`]'s job.
    // `NOINHERIT` is the exception: not a privilege attribute, so a `CREATEROLE`
    // owner may set it, and it stops the role picking up anything from a group
    // it is later added to.
    Ok(vec![format!(
        "DO $$ BEGIN \
           IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = {name}) THEN \
             EXECUTE format('ALTER ROLE %I WITH LOGIN PASSWORD %L', {name}, {pw}); \
           ELSE \
             EXECUTE format('CREATE ROLE %I WITH LOGIN PASSWORD %L', {name}, {pw}); \
           END IF; \
           EXECUTE format('ALTER ROLE %I WITH NOINHERIT', {name}); \
         END $$",
        name = quote_literal(role),
        pw = pw,
    )])
}

/// Let `role` open a connection to this database and use temp tables.
///
/// A role created by `CREATE ROLE` holds no `CONNECT`, so it authenticates and
/// is then refused with `permission denied for database`. Provider-created
/// roles were granted it implicitly, which is why this only surfaced once role
/// creation moved into SQL.
///
/// `current_database()` rather than a name passed in: this always runs on a
/// connection to the tenant database, and threading the name through would let
/// the two disagree.
/// `TEMPORARY` is not a widening: a temp table lives in a per-session schema,
/// is invisible to every other session, and vanishes on disconnect — it grants
/// no access to any existing object. The semantic query engine materialises
/// intermediates that way, so without it the analyst authenticates, resolves
/// the right tables, and then fails every real question with `permission denied
/// to create temporary tables` — which reads as a broken question rather than a
/// missing grant. `platform.rs` revokes ALL from PUBLIC, so nothing hands it
/// out implicitly.
pub fn grant_connect_sql(role: &str) -> Result<String, SchemaError> {
    validate_name(role)?;
    Ok(format!(
        "DO $$ BEGIN \
           EXECUTE format('GRANT CONNECT, TEMPORARY ON DATABASE %I TO %I', \
                          current_database(), {name}); \
         END $$",
        name = quote_literal(role)
    ))
}

/// Let `role` create schemas in this database.
///
/// **Pipeline writers only**, and only because Airway insists on it: its
/// Postgres destination issues `CREATE SCHEMA IF NOT EXISTS <dataset>` on every
/// load, and Postgres checks `CREATE` on the *database* before the
/// `IF NOT EXISTS` short-circuit — so it fails even though Oxy already created
/// `raw_<source>` during provisioning. Airway is a separate repo
/// (`oxy-hq/airway-internal`), so the alternative is that Airway cannot target
/// a per-org OLTP database at all.
///
/// What this costs, stated plainly: a pipeline writer can create *additional*
/// schemas in its own tenant's database. What it does not cost — and this is
/// the boundary that matters — is any access to a schema it was not granted:
/// creating `raw_other` gives it nothing in `app_bookings`, and a different
/// tenant is a different database entirely.
///
/// App writers never get this. Their tables come from migrations, which run as
/// the owner.
pub fn grant_schema_creation_sql(role: &str) -> Result<String, SchemaError> {
    validate_name(role)?;
    Ok(format!(
        "DO $$ BEGIN \
           EXECUTE format('GRANT CREATE ON DATABASE %I TO %I', current_database(), {name}); \
         END $$",
        name = quote_literal(role)
    ))
}

/// The authority a confined role must not hold: display name, `pg_roles` column.
///
/// **One list, two readers.** [`assert_confined_sql`] refuses a credential
/// carrying any of these, and `oxy oltp audit` reports on them — and for a
/// while they were different lists, so the command built to vouch for
/// containment printed `risk: none` and exited 0 for a role provisioning would
/// have refused outright. Anything added here reaches both.
///
/// The set is Neon's, read from their open-source `compute_ctl`
/// (`compute_tools/src/spec_apply.rs`), which creates a spec role as
/// `CREATE ROLE x INHERIT CREATEROLE CREATEDB BYPASSRLS REPLICATION IN ROLE
/// neon_superuser` — four attributes plus a membership.
///
/// **Every column here must be `boolean`.** `oxy oltp audit` reads them
/// positionally as `row.get::<_, bool>(..)`, so a non-boolean addition
/// (`rolconnlimit`, say) compiles and then panics at runtime in the audit,
/// while `assert_confined_sql` would fail with a SQL type error. The tuple
/// cannot express that constraint; this sentence has to.
pub const CONFINEMENT_ATTRIBUTES: &[(&str, &str)] = &[
    ("superuser", "rolsuper"),
    ("createdb", "rolcreatedb"),
    ("createrole", "rolcreaterole"),
    ("bypassrls", "rolbypassrls"),
    // Streams the entire WAL — every other writer's rows — reading PAST table
    // ACLs rather than through them. The one this list was missing.
    ("replication", "rolreplication"),
];

/// Raise unless `role` is exactly as confined as [`ensure_login_role_sql`] made
/// it.
///
/// Run **after** creating a role, as the owner. Checks every attribute in
/// [`CONFINEMENT_ATTRIBUTES`] and, just as importantly, group membership — the
/// Neon failure was not an attribute at all but an implicit
/// `GRANT neon_superuser`, which no `rolsuper` check would have caught.
pub fn assert_confined_sql(role: &str) -> Result<String, SchemaError> {
    validate_name(role)?;
    let name = quote_literal(role);
    let attrs = CONFINEMENT_ATTRIBUTES
        .iter()
        .map(|(label, column)| {
            format!("SELECT '{label}' AS a FROM pg_roles WHERE rolname = {name} AND {column}")
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    Ok(format!(
        "DO $$ DECLARE bad text; BEGIN \
           SELECT string_agg(a, ', ') INTO bad FROM ( \
             {attrs} \
             UNION ALL SELECT 'member of ' || g.rolname FROM pg_auth_members m \
               JOIN pg_roles g ON g.oid = m.roleid \
               JOIN pg_roles u ON u.oid = m.member \
               WHERE u.rolname = {name} \
           ) t; \
           IF bad IS NOT NULL THEN \
             RAISE EXCEPTION 'role % is not confined: %', {name}, bad \
               USING ERRCODE = 'OXY01', HINT = 'the provider granted authority CREATE ROLE did not ask for; \
                             Oxy will not hand out this credential'; \
           END IF; \
         END $$"
    ))
}

/// Raise unless `schema` is owned by `owner`.
///
/// **`CREATE SCHEMA IF NOT EXISTS` does not assert ownership.** If the schema
/// already exists it is a no-op, and whoever created it stays its owner — so a
/// pipeline writer, which holds `CREATE ON DATABASE` by design, can create
/// `app_bookings` before that app is ever provisioned and remain its owner
/// while Oxy layers the app writer's grants on top. The schema owner may read,
/// alter and drop everything in it, so the app's data would sit inside a
/// namespace another writer controls, and every grant Oxy issued would look
/// correct.
///
/// Ownership cannot simply be taken: `ALTER SCHEMA … OWNER TO` requires the
/// executing role to be a member of the new owner, which the tenant owner is
/// not on every provider. So this refuses instead — a provision that stops with
/// `OXY02` is recoverable by hand, and one that silently continues is a
/// containment boundary that exists only on paper.
pub fn assert_schema_owned_sql(schema: &str, owner: &str) -> Result<String, SchemaError> {
    validate_name(schema)?;
    validate_name(owner)?;
    let s = quote_literal(schema);
    let o = quote_literal(owner);
    Ok(format!(
        "DO $$ DECLARE actual text; BEGIN            SELECT pg_get_userbyid(n.nspowner) INTO actual              FROM pg_namespace n WHERE n.nspname = {s};            IF actual IS NOT NULL AND actual <> {o} THEN              RAISE EXCEPTION 'schema % is owned by %, not %', {s}, actual, {o}                USING ERRCODE = 'OXY02', HINT = 'another role created this schema first;                              Oxy will not grant into a namespace it does not own';            END IF;          END $$"
    ))
}

/// Statements to remove `role`, grouped by the connection each MUST run on — the
/// split is load-bearing, not cosmetic.
///
/// `REASSIGN OWNED` / `DROP OWNED` are **per-database**: they touch only objects
/// in the database the session is connected to (plus cluster-shared ones). So
/// they have to run on the TENANT connection — issued against a shared cluster's
/// *admin* database they reach nothing the writer owns, and a role that still
/// owns objects then wedges `DROP ROLE` on `2BP01` on every retry. `DROP ROLE`
/// is cluster-global and, on a shared cluster, only the superuser may drop a
/// superuser-created role, so it rides the admin connection. And the owner must
/// first be granted membership in the role (admin connection) or `DROP OWNED`
/// refuses it `42501`. This is the same split [`super::provisioner`]'s
/// `strip_role_dependencies` proved; a single batch on one connection is the bug.
///
/// A plain `DROP ROLE` fails while the role owns objects, which is exactly the
/// state a writer is in — it may own tables it created.
#[derive(Debug)]
pub struct RoleDropPlan {
    /// Run FIRST, on the role-admin connection: membership so the owner may act
    /// on the role's objects. **No undo** — if a later step fails, the owner
    /// keeps membership in the writer role indefinitely (harmless in privilege
    /// terms, since the owner already dominates it, but `assert_confined_sql`
    /// checks what the role is a member OF, not what is a member of it, so
    /// `oxy oltp audit` won't surface it). Same trade `strip_role_dependencies`
    /// makes.
    pub admin_pre: Vec<String>,
    /// Run on the TENANT database as the owner: reassign what the role owns to
    /// the owner, then drop its grants and defaults.
    pub tenant: Vec<String>,
    /// Run LAST, on the role-admin connection: drop the now-dependency-free role.
    pub admin_post: Vec<String>,
}

pub fn drop_role_plan(role: &str, owner: &str) -> Result<RoleDropPlan, SchemaError> {
    validate_name(role)?;
    validate_name(owner)?;
    let r = quote_ident(role);
    let o = quote_ident(owner);
    Ok(RoleDropPlan {
        admin_pre: vec![format!("GRANT {r} TO {o}")],
        tenant: vec![
            format!("REASSIGN OWNED BY {r} TO {o}"),
            format!("DROP OWNED BY {r}"),
        ],
        admin_post: vec![format!("DROP ROLE IF EXISTS {r}")],
    })
}

/// Percent-encode a DSN userinfo component.
///
/// A generated password reaches Postgres inside a URI, where `@ : / ? # & = +
/// %` all change how the string parses. An unencoded `@` makes libpq read the
/// rest of the password as the hostname — which fails as a TLS error against a
/// nonexistent host, so it does not look like a credential problem at all.
///
/// Everything outside RFC 3986's unreserved set is escaped, which is stricter
/// than userinfo strictly requires and costs nothing.
pub fn encode_userinfo(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the generator emitted `@ # ? % & = +`, and every DSN builder
    /// interpolated the password raw. libpq then read everything after the `@`
    /// as the host and failed with `SSL error: ssl/tls alert illegal parameter`
    /// against a host that did not exist — which reads as a TLS problem, not a
    /// credential one. Two boundary test runs were scored against connections
    /// that had never authenticated.
    #[test]
    fn a_password_survives_being_put_in_a_dsn() {
        for _ in 0..64 {
            let pw = generate_password();
            let dsn = format!("postgres://u:{}@host:5432/db", encode_userinfo(&pw));
            let cfg: tokio_postgres::Config = dsn.parse().expect("must parse as a DSN");
            assert_eq!(
                cfg.get_password(),
                Some(pw.as_bytes()),
                "password must survive the round trip intact: {pw}"
            );
        }
    }

    #[test]
    fn encoding_escapes_everything_that_changes_uri_parsing() {
        assert_eq!(encode_userinfo("a@b"), "a%40b");
        assert_eq!(encode_userinfo("a:b/c?d#e"), "a%3Ab%2Fc%3Fd%23e");
        assert_eq!(encode_userinfo("safe-._~AZ09"), "safe-._~AZ09");
    }

    #[test]
    fn a_generated_password_satisfies_a_provider_policy() {
        let pw = generate_password();
        assert_eq!(pw.len(), PASSWORD_BYTES);
        assert!(pw.chars().any(|c| c.is_ascii_uppercase()), "{pw}");
        assert!(pw.chars().any(|c| c.is_ascii_lowercase()), "{pw}");
        assert!(pw.chars().any(|c| c.is_ascii_digit()), "{pw}");
        assert!(
            pw.chars().any(|c| !c.is_ascii_alphanumeric()),
            "needs a symbol, Neon rejects passwords without one: {pw}"
        );
        assert!(!pw.contains('\''), "must not need escaping: {pw}");
        assert_ne!(generate_password(), generate_password());
    }

    /// The confinement list is exactly these five, and all five reach the SQL.
    ///
    /// **The expected set is spelled out here on purpose.** The obvious
    /// version of this test iterates `CONFINEMENT_ATTRIBUTES` and asserts each
    /// entry appears in the generated SQL — which cannot fail when an entry is
    /// DELETED, because the loop simply stops checking it. I wrote that
    /// version first and a mutation removing `rolreplication` passed it. An
    /// independent literal is the only shape that catches a removal, and
    /// removal is the failure that matters: every attribute here is one a
    /// provider can confer and Oxy refuses to hand out.
    ///
    /// Adding an attribute should fail this test. Read the new entry, satisfy
    /// yourself it is a `boolean` column (the audit reads them as `bool`), then
    /// add it below.
    #[test]
    fn the_confinement_list_is_exactly_these_five_and_all_reach_the_sql() {
        let expected = [
            ("superuser", "rolsuper"),
            ("createdb", "rolcreatedb"),
            ("createrole", "rolcreaterole"),
            ("bypassrls", "rolbypassrls"),
            ("replication", "rolreplication"),
        ];
        assert_eq!(
            CONFINEMENT_ATTRIBUTES,
            &expected[..],
            "the confinement list changed — see this test's doc before updating it"
        );

        let sql = assert_confined_sql("some_role").expect("build");
        for (label, column) in expected {
            assert!(
                sql.contains(column),
                "{column} missing from the check: {sql}"
            );
            assert!(sql.contains(label), "{label} missing from the check: {sql}");
        }
    }

    #[test]
    fn no_privilege_attribute_is_named_and_noinherit_is() {
        let sql = ensure_login_role_sql("app_bookings_rw", "pw")
            .unwrap()
            .remove(0);
        // Only what a CREATEROLE owner may set. The superuser-gated three
        // (NOSUPERUSER / NOREPLICATION / NOBYPASSRLS) are off by default and
        // naming them makes Postgres refuse the whole statement — verified
        // against live Neon, where it failed the provision outright.
        // NOINHERIT is the only one a CREATEROLE owner may set.
        assert!(sql.contains("NOINHERIT"), "{sql}");
        // Every privilege attribute is gated on holding it yourself, even to
        // clear it. Naming any of these fails the provision on some owner —
        // verified on both a local cluster and Neon. They are off by default;
        // `assert_confined_sql` is what proves it.
        for gated in [
            "NOSUPERUSER",
            "NOCREATEDB",
            "NOCREATEROLE",
            "NOREPLICATION",
            "NOBYPASSRLS",
        ] {
            assert!(!sql.contains(gated), "{gated} must not be set here: {sql}");
        }
    }

    #[test]
    fn a_new_role_is_granted_connect_or_it_cannot_log_in_at_all() {
        let sql = grant_connect_sql("oxy_analyst_ro").unwrap();
        assert!(
            sql.contains("GRANT CONNECT, TEMPORARY ON DATABASE"),
            "{sql}"
        );
        // Named from the session, so it cannot target the wrong database.
        assert!(sql.contains("current_database()"), "{sql}");
        assert!(grant_connect_sql("bad name").is_err());
    }

    #[test]
    fn the_confinement_check_looks_at_membership_not_just_attributes() {
        let sql = assert_confined_sql("oxy_analyst_ro").unwrap();
        // The Neon failure was an implicit GRANT, not an attribute — a check
        // that only read `rolsuper` would have passed it.
        assert!(
            sql.contains("pg_auth_members"),
            "must inspect membership: {sql}"
        );
        assert!(sql.contains("RAISE EXCEPTION"), "must fail closed: {sql}");
    }

    #[test]
    fn an_injection_shaped_role_name_is_rejected_before_any_sql() {
        assert!(ensure_login_role_sql("app'; DROP DATABASE x; --", "pw").is_err());
        assert!(assert_confined_sql("bad name").is_err());
        assert!(drop_role_plan("ok_role", "bad owner").is_err());
    }

    #[test]
    fn dropping_reassigns_on_the_tenant_and_drops_the_role_on_admin() {
        let plan = drop_role_plan("app_bookings_rw", "oxy_owner").unwrap();
        // Membership first (admin), or DROP OWNED is refused 42501.
        assert!(plan.admin_pre[0].starts_with("GRANT"), "{plan:?}");
        // A writer owns the tables it created; DROP ROLE alone would fail — so
        // REASSIGN/DROP OWNED run on the tenant connection (per-database) first.
        assert!(plan.tenant[0].starts_with("REASSIGN OWNED BY"), "{plan:?}");
        assert!(plan.tenant[1].starts_with("DROP OWNED BY"), "{plan:?}");
        // DROP ROLE is cluster-global — the admin connection, last.
        assert!(
            plan.admin_post[0].contains("DROP ROLE IF EXISTS"),
            "{plan:?}"
        );
    }
}
