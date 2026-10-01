//! The statements the rewrite replaces rather than redirects: `CREATE
//! SCHEMA`, `DROP` and `TRUNCATE`.

use sqlparser::ast::{ObjectType, SchemaName, Statement, Truncate};

use super::Refused;
use super::ShadowState;
use super::names;
use super::rewrite::{Effect, Rewriter, not_allowed};

impl Rewriter<'_> {
    /// `CREATE SCHEMA S` is not sent; the host ensures `S`'s stand-in.
    pub(super) fn create_schema(&mut self, statement: &Statement) -> Result<(), Refused> {
        let Statement::CreateSchema {
            schema_name: SchemaName::Simple(name),
            clone: None,
            ..
        } = statement
        else {
            return Err(not_allowed("CREATE SCHEMA with AUTHORIZATION or CLONE"));
        };
        let live = names::schema(name, self.opts)?;
        let preview = self.ns.schema_for(&live)?;
        self.ensure_schema(live, preview);
        Ok(())
    }

    /// One `DROP` per name. The live table stays; the preview's copy goes,
    /// and the table reads as dropped for the rest of the run.
    pub(super) fn drop(&mut self, statement: &Statement) -> Result<(), Refused> {
        let Statement::Drop {
            object_type,
            names,
            if_exists,
            cascade,
            temporary,
            table,
            ..
        } = statement
        else {
            unreachable!("drop() is only called for DROP");
        };
        if !matches!(object_type, ObjectType::Table | ObjectType::View) {
            return Err(not_allowed(&format!("DROP {object_type}")));
        }
        if *cascade || *temporary || table.is_some() {
            return Err(not_allowed("DROP with CASCADE, TEMPORARY or ON"));
        }
        for name in names {
            let mut preview = name.clone();
            let target = self.target(&mut preview, "DROP")?;
            let known = self.shadow.state(&target.live).is_some();
            // Prod's DROP without IF EXISTS fails on a missing table; with no
            // copy, the live table is the one that must exist.
            if !known && !*if_exists {
                self.sent
                    .push(format!("SELECT * FROM {} LIMIT 0", target.live_name));
            }
            let if_exists = if known { *if_exists } else { true };
            self.sent.push(format!(
                "DROP {object_type} {}{preview}",
                if if_exists { "IF EXISTS " } else { "" }
            ));
            self.record(Effect::Set(target.live, ShadowState::Dropped));
        }
        Ok(())
    }

    /// `TRUNCATE S.t` empties the preview's copy, making an empty one from
    /// the live table's columns when there is none.
    pub(super) fn truncate(&mut self, truncate: &Truncate) -> Result<(), Refused> {
        if truncate.cascade.is_some() || truncate.partitions.is_some() {
            return Err(not_allowed("TRUNCATE with CASCADE, RESTRICT or PARTITION"));
        }
        for table in &truncate.table_names {
            let mut preview = table.name.clone();
            let target = self.target(&mut preview, "TRUNCATE")?;
            let state = self.shadow.state(&target.live);
            match state {
                None => self.sent.push(format!(
                    "CREATE OR REPLACE TABLE {preview} AS SELECT * FROM {} LIMIT 0",
                    target.live_name
                )),
                Some(_) => self.sent.push(format!("TRUNCATE {preview}")),
            }
            // Truncating a dropped table fails, as in prod, and changes nothing.
            if state != Some(ShadowState::Dropped) {
                self.record(Effect::Set(target.live, ShadowState::Shadow));
            }
        }
        Ok(())
    }
}
