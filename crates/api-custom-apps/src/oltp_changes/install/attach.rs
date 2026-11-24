//! Who attaches a table's capture triggers, and who drops them: the role that
//! owns the table. `CREATE TRIGGER` needs only the `TRIGGER` privilege, but
//! `ALTER TABLE … ENABLE ALWAYS TRIGGER` and `DROP TRIGGER` need ownership.
//!
//! Two kinds of table live in `app_<writer>`. The app's own migrations run as
//! its writer (`custom_apps_migrations`), so the tables they create are the
//! writer's. A workspace's `schemas/*.sql` run as the store's owner
//! (`oxy_oltp::migrator`), so theirs are the owner's, and the writer holds
//! only the grants `oxy_oltp::schema::reconcile_migration_grants_sql` gives
//! it. So each table's triggers go on, and come off, on the session of the
//! role that owns it ([`attacher`]). A table any other role owns can be
//! captured from neither session: the install refuses it, as the author's to
//! fix, naming the table and its owner, before anything changes ([`decide`]).

use tokio_postgres::Client;

use super::plan::{Installed, refusal};
use super::{InstallError, TableSql, pg_infra};

/// The schema, name and owner of the table with oid `$1`, if it still exists.
/// Its owner decides which session attaches its triggers; a removed table's
/// triggers are dropped where it is now, which after a rename (on a store
/// without the DDL guard) is not where its row says.
pub(super) const LIVE_TABLE_SQL: &str = "SELECT n.nspname::text AS schema_name, \
       c.relname::text AS table_name, pg_catalog.pg_get_userbyid(c.relowner)::text AS owner \
       FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
      WHERE c.oid = $1";

/// The role the owner's session runs as, in the store's own spelling: a
/// table's owner is compared with this, never with a name built here, since
/// a shared-cluster provider qualifies role names.
pub(super) const CURRENT_USER_SQL: &str = "SELECT current_user::text";

/// The session that attaches and drops a table's capture triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Attacher {
    /// The app's writer owns the table: its migrations created it.
    Writer,
    /// The store's owner owns the table: a workspace's `schemas/*.sql`
    /// created it in the app's schema.
    Owner,
}

impl Attacher {
    /// Who the session runs as, for what a failure says it was doing.
    pub(super) fn role(self) -> &'static str {
        match self {
            Self::Writer => "the app's writer",
            Self::Owner => "the store's owner",
        }
    }
}

/// The roles of the two sessions an install holds, as the store spells them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Roles<'a> {
    /// The writer's real role (`WriterConnection::role`), qualified on a
    /// shared-cluster provider.
    pub(super) writer: &'a str,
    /// The store's owner, as its own session reports it ([`store_owner`]).
    pub(super) owner: &'a str,
}

/// Which session attaches and drops the triggers of `table`, which the role
/// `table_owner` owns: the writer's when the writer owns it, the store
/// owner's when the store's owner does. `Err` is the refusal for any other
/// owner, naming the table and that role.
pub(super) fn attacher(
    table: &str,
    table_owner: &str,
    roles: Roles<'_>,
) -> Result<Attacher, String> {
    if table_owner == roles.writer {
        Ok(Attacher::Writer)
    } else if table_owner == roles.owner {
        Ok(Attacher::Owner)
    } else {
        Err(format!(
            "oltpChanges table \"{table}\" is owned by the role \"{table_owner}\", and only a \
             table's owner can attach or drop its capture triggers, which the platform does as \
             the app's writer or the store's owner: create the table in the app's migrations, \
             so the writer owns it, or give it back to one of those two roles"
        ))
    }
}

/// A table as the catalog has it now ([`LIVE_TABLE_SQL`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiveTable {
    pub(super) schema: String,
    pub(super) table: String,
    pub(super) owner: String,
}

/// One declared table's install, with the session that attaches its triggers.
pub(super) struct Attach<'a> {
    pub(super) sql: &'a TableSql,
    pub(super) by: Attacher,
}

/// One table no longer declared, with its triggers: where they are now and
/// the session that drops them. `None` when the table is gone (its triggers
/// went with it) or left the writer's schema.
pub(super) struct Detach<'a> {
    pub(super) row: &'a Installed,
    pub(super) triggers: Option<(LiveTable, Attacher)>,
}

/// What an install attaches and detaches, each table's owner already read.
pub(super) struct Steps<'a> {
    pub(super) install: Vec<Attach<'a>>,
    pub(super) remove: Vec<Detach<'a>>,
}

/// The role the owner's session runs as ([`CURRENT_USER_SQL`]).
pub(super) async fn store_owner(owner: &Client) -> Result<String, InstallError> {
    let doing = "read the store owner's role";
    owner
        .query_one(CURRENT_USER_SQL, &[])
        .await
        .and_then(|row| row.try_get(0))
        .map_err(|e| pg_infra(doing, &e))
}

/// Every table's owner, read on the owner's session and checked against
/// `roles` before anything changes: the tables `tables` installs, then those
/// `removed` takes down, in `schema` (`app_<writer>`). `Err` is the author's
/// refusal, naming each table another role owns.
pub(super) async fn decide<'a>(
    owner: &Client,
    roles: Roles<'_>,
    tables: &'a [TableSql],
    removed: &'a [Installed],
    schema: &str,
) -> Result<Steps<'a>, InstallError> {
    let mut decider = Decider {
        owner,
        roles,
        refusals: Vec::new(),
    };
    let mut install = Vec::with_capacity(tables.len());
    for sql in tables {
        if let Some(by) = decider.install(sql).await? {
            install.push(Attach { sql, by });
        }
    }
    let mut remove = Vec::with_capacity(removed.len());
    for row in removed {
        let triggers = decider.remove(row, schema).await?;
        remove.push(Detach { row, triggers });
    }
    decider.finish(Steps { install, remove })
}

/// Reads each table's owner and decides its attacher, keeping every refusal
/// so that one names them all.
struct Decider<'c, 'r> {
    owner: &'c Client,
    roles: Roles<'r>,
    refusals: Vec<String>,
}

impl Decider<'_, '_> {
    /// The attacher of a table about to be installed. Its shape was read on
    /// this session a moment ago, so a table gone now was dropped meanwhile:
    /// the same promote again refuses it as missing.
    async fn install(&mut self, sql: &TableSql) -> Result<Option<Attacher>, InstallError> {
        let live = live_table(self.owner, sql.relid).await?.ok_or_else(|| {
            InstallError::Infra(format!(
                "table {:?} was dropped while its capture was being installed",
                sql.table
            ))
        })?;
        Ok(self.check(&sql.table, &live.owner))
    }

    /// Where a removed table's triggers are and who drops them, if it still
    /// has any in `schema`.
    async fn remove(
        &mut self,
        row: &Installed,
        schema: &str,
    ) -> Result<Option<(LiveTable, Attacher)>, InstallError> {
        let live = live_table(self.owner, row.relid).await?;
        let Some(live) = live.filter(|t| t.schema == schema) else {
            return Ok(None);
        };
        Ok(self.check(&row.table, &live.owner).map(|by| (live, by)))
    }

    /// [`attacher`], keeping its refusal.
    fn check(&mut self, table: &str, table_owner: &str) -> Option<Attacher> {
        match attacher(table, table_owner, self.roles) {
            Ok(by) => Some(by),
            Err(refused) => {
                self.refusals.push(refused);
                None
            }
        }
    }

    fn finish<T>(self, steps: T) -> Result<T, InstallError> {
        if self.refusals.is_empty() {
            Ok(steps)
        } else {
            Err(InstallError::Author(refusal(&self.refusals)))
        }
    }
}

/// The table with oid `relid` as the catalog has it now, if it still exists.
async fn live_table(owner: &Client, relid: u32) -> Result<Option<LiveTable>, InstallError> {
    let doing = "read a captured table's name and owner";
    let Some(row) = owner
        .query_opt(LIVE_TABLE_SQL, &[&relid])
        .await
        .map_err(|e| pg_infra(doing, &e))?
    else {
        return Ok(None);
    };
    let read = |row: &tokio_postgres::Row| -> Result<LiveTable, tokio_postgres::Error> {
        Ok(LiveTable {
            schema: row.try_get("schema_name")?,
            table: row.try_get("table_name")?,
            owner: row.try_get("owner")?,
        })
    };
    read(&row).map(Some).map_err(|e| pg_infra(doing, &e))
}

#[cfg(test)]
mod tests {
    use super::{Attacher, CURRENT_USER_SQL, LIVE_TABLE_SQL, Roles, attacher};

    /// A local store's roles: each qualified by its tenant's tag.
    const ROLES: Roles<'static> = Roles {
        writer: "app_leases_rw_1a2b3c4d",
        owner: "oxy_org_5e6f_owner",
    };

    /// A table the app's migrations made is the writer's, so the writer
    /// attaches its triggers; one a workspace's schemas made in the app's
    /// schema is the owner's, so the owner does.
    #[test]
    fn the_tables_owner_attaches_its_triggers() {
        assert_eq!(
            attacher("leases", ROLES.writer, ROLES),
            Ok(Attacher::Writer)
        );
        assert_eq!(attacher("leases", ROLES.owner, ROLES), Ok(Attacher::Owner));
    }

    /// Any other owner is refused, naming the table and the role: neither
    /// session could enable or drop a trigger on it.
    #[test]
    fn a_table_another_role_owns_is_refused_naming_it() {
        let refused = attacher("leases", "app_other_rw_1a2b3c4d", ROLES)
            .expect_err("neither session owns the table");
        assert!(refused.contains("\"leases\""), "{refused}");
        assert!(refused.contains("\"app_other_rw_1a2b3c4d\""), "{refused}");
        assert!(refused.contains("app's migrations"), "{refused}");
    }

    /// Names are compared as the store spells them, never loosely: the
    /// writer's unqualified name is another role on a store that qualifies.
    #[test]
    fn role_names_are_compared_exactly() {
        for owner in ["app_leases_rw", "APP_LEASES_RW_1A2B3C4D", "oxy_org_5e6f"] {
            assert!(attacher("leases", owner, ROLES).is_err(), "{owner}");
        }
    }

    #[test]
    fn each_session_says_who_it_runs_as() {
        assert_eq!(Attacher::Writer.role(), "the app's writer");
        assert_eq!(Attacher::Owner.role(), "the store's owner");
    }

    /// The owner comes from the catalog and the store owner from its own
    /// session, both as text.
    #[test]
    fn owners_are_read_from_the_store() {
        assert!(LIVE_TABLE_SQL.contains("pg_catalog.pg_get_userbyid(c.relowner)::text AS owner"));
        assert!(LIVE_TABLE_SQL.contains("WHERE c.oid = $1"));
        assert_eq!(CURRENT_USER_SQL, "SELECT current_user::text");
    }
}
