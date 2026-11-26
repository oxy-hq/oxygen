//! What a copy reads of its table: the captured columns and the primary key
//! of the shape version its baseline began under, and the key's types and
//! btree operators, read in the writer's pinned session.

use oxy_oltp::changes::{KEY_TYPES_SQL, KeyOrder, SnapshotShape, pin_session_sql};
use tokio_postgres::Client;

use super::{Open, Sessions};
use crate::oltp_changes::relay::store::{self, pg};
use crate::oltp_changes::relay::{BASELINE_STATEMENT_TIMEOUT, BEGIN_LOCK_TIMEOUT};

/// What a copy reads of its table, owned.
pub(super) struct Shape {
    captured: Vec<String>,
    primary_key: Vec<String>,
    key_types: Vec<String>,
    key_order: Vec<KeyOrder>,
}

impl Shape {
    /// Whether every key column's type has its btree operators to copy by.
    pub(super) fn ordered(&self) -> bool {
        self.key_order
            .iter()
            .all(|o| !(o.lt.is_empty() || o.eq.is_empty() || o.ge.is_empty() || o.gt.is_empty()))
    }

    pub(super) fn snapshot(&self) -> SnapshotShape<'_> {
        SnapshotShape {
            captured: &self.captured,
            primary_key: &self.primary_key,
            key_types: &self.key_types,
            key_order: &self.key_order,
        }
    }
}

/// One key column as [`KEY_TYPES_SQL`] reads it: its name, its type, and its
/// type's btree operators, empty when it has no default class, which
/// `chunk_read_sql` then refuses.
pub(super) struct TypedColumn {
    pub name: String,
    pub ty: String,
    pub order: KeyOrder,
}

/// `b`'s shape version as `store::shapes` records it, with its key's types
/// and operators read by [`KEY_TYPES_SQL`] in the writer's session
/// ([`typed_key`]): never from the recorded columns' type text, which depends
/// on the search path and carries typmods.
pub(super) async fn read(s: &mut Sessions<'_>, b: &Open) -> Result<Shape, String> {
    let version = b.schema_version.ok_or("a copying baseline has no shape")?;
    let shapes = store::shapes(&*s.owner, &s.name).await?;
    let recorded = shapes
        .get(&b.table)
        .and_then(|versions| versions.iter().find(|v| v.schema_version == version))
        .ok_or_else(|| format!("no recorded shape for schema_version {version}"))?;
    let relid = u32::try_from(b.relid).map_err(|_| format!("relid {} is not an oid", b.relid))?;
    let named = typed_key(&mut s.writer, relid, &recorded.primary_key).await?;
    let (key_types, key_order) = key_types(&recorded.primary_key, named)?;
    Ok(Shape {
        captured: recorded.captured.iter().map(|c| c.name.clone()).collect(),
        primary_key: recorded.primary_key.clone(),
        key_types,
        key_order,
    })
}

/// `relid`'s `key` columns as [`KEY_TYPES_SQL`] reads them, in a writer
/// transaction pinned as every step's is ([`pin_session_sql`]).
async fn typed_key(
    writer: &mut Client,
    relid: u32,
    key: &[String],
) -> Result<Vec<TypedColumn>, String> {
    let doing = "read the key's types";
    let t = writer.transaction().await.map_err(pg(doing))?;
    let pin = pin_session_sql(BEGIN_LOCK_TIMEOUT, BASELINE_STATEMENT_TIMEOUT);
    t.batch_execute(&pin).await.map_err(pg(doing))?;
    let rows = t
        .query(KEY_TYPES_SQL, &[&relid, &key])
        .await
        .map_err(pg(doing))?;
    t.commit().await.map_err(pg(doing))?;
    rows.iter()
        .map(typed_column)
        .collect::<Result<_, tokio_postgres::Error>>()
        .map_err(pg(doing))
}

/// A row of [`KEY_TYPES_SQL`].
fn typed_column(row: &tokio_postgres::Row) -> Result<TypedColumn, tokio_postgres::Error> {
    let op = |name: &str| -> Result<String, tokio_postgres::Error> {
        Ok(row.try_get::<_, Option<String>>(name)?.unwrap_or_default())
    };
    Ok(TypedColumn {
        name: row.try_get("attname")?,
        ty: row.try_get("type")?,
        order: KeyOrder {
            lt: op("lt")?,
            eq: op("eq")?,
            ge: op("ge")?,
            gt: op("gt")?,
        },
    })
}

/// The key's types and operators in its order, from [`KEY_TYPES_SQL`]'s
/// rows, which come in that order. A key column without its row refuses the
/// copy.
pub(super) fn key_types(
    primary_key: &[String],
    named: Vec<TypedColumn>,
) -> Result<(Vec<String>, Vec<KeyOrder>), String> {
    let names: Vec<&str> = named.iter().map(|c| c.name.as_str()).collect();
    if names != primary_key {
        return Err(format!(
            "the key's columns {primary_key:?} are not all there to type: found {names:?}"
        ));
    }
    Ok(named.into_iter().map(|c| (c.ty, c.order)).unzip())
}
