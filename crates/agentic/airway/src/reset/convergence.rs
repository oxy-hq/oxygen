//! Does re-pulling a window converge, or duplicate?
//!
//! The question a **cursor** reset has to answer and a **schema** reset never
//! does. Dropping the tables first makes every disposition safe by
//! construction; rewinding a cursor while the rows stay put makes the answer
//! depend entirely on what the destination does with a row it has already seen.
//!
//! The oracle is the pipeline's **stored** [`Schema`] — the one already in the
//! `airway_workspace_pipeline_state` row next to the cursors. It records what
//! actually landed (per-table `write_disposition`, per-column `primary_key` /
//! `merge_key`), so the judgement needs no live destination, no credential and
//! no source config. A pipeline that has never provisioned has no schema, and
//! therefore no rows to duplicate.
//!
//! Judged against the airhouse write path, which is the one oxy ships:
//!
//! | disposition       | key it dedups on                                   | converges? |
//! | ----------------- | -------------------------------------------------- | ---------- |
//! | `Replace`         | — (the load replaces the table wholesale)           | yes        |
//! | `Append`          | —                                                   | **no**     |
//! | `Merge`           | `primary_key`                                       | iff keyed  |
//! | `Replacing`       | `primary_key` (empty is rejected at migrate time)   | iff keyed  |
//! | `ReplaceByParent` | `merge_key`, falling back to `primary_key`          | iff keyed  |
//!
//! The keyed-ness test is not decoration. `AirhouseDestination::upsert_impl`
//! opens with `if pks.is_empty() { return self.load_impl(…) }` — a `Merge`
//! table that declares no `primary_key` column **silently appends**. So the
//! decision is on the *effective key*, never on the disposition's name: of the
//! five, `Merge` is the one whose name actively misleads.

use std::collections::BTreeSet;

use airway::Schema;
use airway::schema::Table;
use airway::types::WriteDisposition;

/// Why re-pulling a window into this table would not converge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonConvergentReason {
    /// `Append`: every re-pulled row is a new row, by definition.
    AppendOnly,
    /// A disposition that dedups on a key, which declares none — so the write
    /// path falls through to a plain append. Carries the disposition because
    /// naming it is the whole point: "it's a Merge table" is exactly the
    /// reasoning this case defeats.
    NoDedupKey(WriteDisposition),
}

/// One table a cursor reset would re-write, and why that is not safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonConvergentTable {
    pub table: String,
    pub reason: NonConvergentReason,
}

impl std::fmt::Display for NonConvergentTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { table, reason } = self;
        match reason {
            NonConvergentReason::AppendOnly => write!(
                f,
                "`{table}` appends with no merge key, so re-pulling would duplicate rows \
                 rather than converge"
            ),
            NonConvergentReason::NoDedupKey(d) => write!(
                f,
                "`{table}` is `{}` with no effective key, which appends — so re-pulling \
                 would duplicate rows rather than converge",
                disposition_name(d)
            ),
        }
    }
}

/// The YAML/serde spelling, so an operator can grep the refusal against a
/// schema dump. `WriteDisposition` derives `Serialize` with
/// `rename_all = "snake_case"` but not `Display`.
fn disposition_name(d: &WriteDisposition) -> &'static str {
    match d {
        WriteDisposition::Append => "append",
        WriteDisposition::Replace => "replace",
        WriteDisposition::Merge => "merge",
        WriteDisposition::ReplaceByParent => "replace_by_parent",
        WriteDisposition::Replacing => "replacing",
    }
}

/// Would re-pulling an already-loaded window into `table` converge?
///
/// Exhaustive on `WriteDisposition` **deliberately**: airway adding a sixth
/// disposition must break this build rather than default into "safe", which is
/// the direction that loses rows quietly.
pub fn table_converges_on_repull(table: &Table) -> Result<(), NonConvergentReason> {
    match &table.write_disposition {
        // The load truncates and rewrites; a re-pull lands the same table.
        WriteDisposition::Replace => Ok(()),
        WriteDisposition::Append => Err(NonConvergentReason::AppendOnly),
        // Both dedup on `primary_key`. `Replacing` additionally refuses an
        // empty key at migrate time, so a landed `Replacing` table is keyed in
        // practice — checked anyway, because "in practice" is not a guarantee
        // and this is the function that decides whether rows survive.
        d @ (WriteDisposition::Merge | WriteDisposition::Replacing) => {
            if table.primary_keys().is_empty() {
                Err(NonConvergentReason::NoDedupKey(d.clone()))
            } else {
                Ok(())
            }
        }
        // Deletes by propagated parent key, then reinserts that parent's whole
        // child set. Mirrors `upsert_impl`'s `ByParent` arm: `merge_key` first,
        // `primary_key` as the fallback for hints that predate it.
        d @ WriteDisposition::ReplaceByParent => {
            if table.merge_keys().is_empty() && table.primary_keys().is_empty() {
                Err(NonConvergentReason::NoDedupKey(d.clone()))
            } else {
                Ok(())
            }
        }
    }
}

/// Which resources' cursors a reset targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorScope {
    /// Every resource holding a cursor in this pipeline's state.
    AllResources,
    /// Named resources only. Names are the **raw** resource names — the keys of
    /// `PipelineState::resource_states`, which is what
    /// [`crate::reset::stored_resource_cursors`] lists.
    Resources(Vec<String>),
}

impl CursorScope {
    /// The resources this scope names, given what the pipeline actually holds.
    pub fn resolve(&self, held: &[String]) -> Vec<String> {
        match self {
            Self::AllResources => held.to_vec(),
            Self::Resources(names) => names.clone(),
        }
    }
}

/// Everything standing between a caller and a cursor reset.
///
/// Both variants are refusals; they are separate because they answer different
/// questions and an operator needs to know which one they would be overriding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorResetRefusal {
    /// A table in scope would duplicate on a re-pull.
    WouldDuplicate(NonConvergentTable),
    /// The scope could not be narrowed to the named resources, so the whole
    /// schema was judged instead. Names the table that forced it.
    ///
    /// `Table::parent` exists on the schema struct and is **never populated**
    /// by the normalizer or by schema inference, so there is no resource→table
    /// edge to follow; the only link is the normalizer's `<root>__<child>`
    /// naming, which a connector's `table_name_mappings` (Toast renames every
    /// nested child) defeats. A table no resource root claims is therefore of
    /// *unknown* ownership, not of no ownership.
    UnknownOwnership { table: String },
}

impl CursorResetRefusal {
    /// Stable machine code for this kind of refusal, for a client that
    /// branches on it rather than on prose. Exhaustive, so a new variant has
    /// to be given its own code instead of borrowing one.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WouldDuplicate(_) => "would_duplicate",
            Self::UnknownOwnership { .. } => "unknown_ownership",
        }
    }

    /// The table this refusal is about.
    pub fn table(&self) -> &str {
        match self {
            Self::WouldDuplicate(t) => &t.table,
            Self::UnknownOwnership { table } => table,
        }
    }
}

impl std::fmt::Display for CursorResetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WouldDuplicate(t) => {
                write!(f, "{t}. Use Reset schema if you intend to drop and rebuild")
            }
            // States the observation, then both readings — an operator who
            // reaches for `force` after reading a bare refusal has been told
            // to override something they were not shown.
            Self::UnknownOwnership { table } => write!(
                f,
                "cannot scope this reset: the stored schema holds `{table}`, which no \
                 resource root claims, so the resource→table mapping is unknown \
                 (`Table::parent` is unpopulated and a connector may rename nested \
                 children). Judging the whole schema instead — either `{table}` belongs \
                 to a resource you did not name, or it belongs to one you did and the \
                 name was remapped. Pass `force` if you know this resource's tables \
                 converge"
            ),
        }
    }
}

/// Does `resource` own `table_name` under the normalizer's naming?
///
/// The root table takes the resource's normalized name; nested arrays become
/// `<root>__<child>`. This is the only link that survives into the stored
/// schema — see [`CursorResetRefusal::UnknownOwnership`] for what it misses.
///
/// `SnakeCase` is hard-coded because it is the only convention a table oxy
/// lands can have been named under, not because it is airway's only one.
/// airway also has `NamingConvention::Direct`, selected per pipeline through
/// `Pipeline::with_normalizer`; oxy's worker builds its pipeline with
/// `Pipeline::new` and never calls it, and no `.airway.yml` key reaches it, so
/// every landed table took airway's default. Were that to change, root tables
/// would stop matching here, every scoped reset would widen to the whole
/// schema, and `UnknownOwnership` would fire on tables that are plainly owned.
/// `the_naming_this_assumes_is_the_one_oxy_lands_with` fails first.
fn resource_owns(resource: &str, table_name: &str) -> bool {
    let root = airway::normalizer::NamingConvention::SnakeCase.normalize(resource);
    table_name == root || table_name.starts_with(&format!("{root}__"))
}

/// Judge a cursor reset: every reason it should not proceed, most specific
/// first.
///
/// `held` is every resource with a cursor in this pipeline's state; it is what
/// an [`AllResources`](CursorScope::AllResources) scope resolves to.
/// `declared` is the spec's `resources:` list, empty when the spec omits it.
///
/// Neither alone is the **attribution universe** — the set of resources whose
/// tables count as accounted for. A table claimed by a resource *outside* the
/// scope does not widen the judgement; only a table claimed by **no** known
/// resource does. The universe is `held ∪ declared ∪ the scope's own names`:
///
/// * `held` alone was the original bug. A resource stops holding a cursor the
///   moment one is cleared, so repeating a scoped reset that just succeeded
///   found its own table unclaimed, widened to the whole schema and refused —
///   blaming a sibling the caller never named. A full-refresh resource that
///   never holds a cursor did the same on every call.
/// * `declared` covers that full-refresh resource, but only where the spec
///   lists `resources:` at all; omitted, it means "whatever the source
///   advertises", which is not knowable without constructing the connector.
/// * The scope's names close the repeat case on every pipeline, and adding
///   them cannot hide anything: a table they claim is in scope, so it is
///   judged regardless.
///
/// What remains unclaimed is a table no known resource's root names — the
/// renamed-nested-child case [`CursorResetRefusal::UnknownOwnership`] exists
/// for, or a resource that is undeclared and holds no cursor.
///
/// Empty means the reset is safe. Order is stable (`BTreeSet` on table name)
/// so a refusal message does not reshuffle between calls over one `HashMap`.
pub fn cursor_reset_refusals(
    schema: Option<&Schema>,
    scope: &CursorScope,
    held: &[String],
    declared: &[String],
) -> Vec<CursorResetRefusal> {
    // No stored schema ⇒ nothing has landed ⇒ nothing to duplicate. This is
    // also the never-provisioned path, where a reset is a formality.
    let Some(schema) = schema else {
        return Vec::new();
    };

    // A reset that names no cursor this pipeline holds clears nothing, so it
    // re-pulls nothing and cannot duplicate anything. Judging it as a
    // narrowing refused the retry of a rewind that had just succeeded — the
    // "refuses its own successful request" shape the attribution universe
    // below exists to end, one step along.
    let in_scope = scope.resolve(held);
    if !in_scope.iter().any(|r| held.contains(r)) {
        return Vec::new();
    }

    let named: &[String] = match scope {
        CursorScope::AllResources => &[],
        CursorScope::Resources(names) => names,
    };
    let universe: BTreeSet<&str> = held
        .iter()
        .chain(declared)
        .chain(named)
        .map(String::as_str)
        .collect();
    let unclaimed: BTreeSet<&str> = schema
        .tables
        .keys()
        .filter(|t| !universe.iter().any(|r| resource_owns(r, t)))
        .map(String::as_str)
        .collect();

    let mut refusals: Vec<CursorResetRefusal> = Vec::new();

    // A list naming every held resource re-pulls every resource, exactly as
    // `AllResources` does, so it is judged the same way — and asserts no
    // narrowing, so it has none to explain. This is what lets a client send
    // the list the operator actually picked instead of `[]`: `[]` resolves
    // against the cursors held *now*, which can include one that appeared
    // after the operator's picker loaded, while the list clears only what
    // was named. Without this, the explicit list drew an `UnknownOwnership`
    // refusal on every whole-pipeline rewind of a connector that renames
    // nested children — a refusal routine enough to train the override.
    let covers_every_held = match scope {
        CursorScope::AllResources => true,
        CursorScope::Resources(names) => !held.is_empty() && held.iter().all(|h| names.contains(h)),
    };

    // Reported only where the caller asked for a narrower scope than they got.
    // Covering every held cursor narrows nothing, so naming the unclaimed
    // tables would be noise about a scope nobody requested.
    if !covers_every_held {
        refusals.extend(
            unclaimed
                .iter()
                .map(|t| CursorResetRefusal::UnknownOwnership {
                    table: (*t).to_string(),
                }),
        );
    }

    // Judged: every table the reset may re-pull. That is each table an
    // in-scope resource claims, plus every unclaimed one — which may belong
    // to a resource in scope (a renamed nested child), so it cannot be
    // excluded, and which a whole-pipeline reset re-pulls regardless. A table
    // claimed only by a resource *outside* the scope is not re-pulled, so it
    // is not judged: a sibling's append table must not refuse a reset that
    // never touches it — nor, under a whole-pipeline scope, must a declared
    // full-refresh resource that holds no cursor to clear.
    //
    // This is safe only because a re-pull writes nothing but tables
    // `resource_owns` attributes to the resource being re-pulled — the same
    // name-prefix rule the ownership universe rests on, now load-bearing here
    // too. Were a resource's re-pull to write a table that rule attributes to
    // someone else, that table would go unjudged and a scoped reset could
    // duplicate into it. Weaken `resource_owns` and both places move.
    let judged: BTreeSet<&str> = schema
        .tables
        .keys()
        .map(String::as_str)
        .filter(|t| unclaimed.contains(t) || in_scope.iter().any(|r| resource_owns(r, t)))
        .collect();

    for name in judged {
        let Some(table) = schema.tables.get(name) else {
            continue;
        };
        if let Err(reason) = table_converges_on_repull(table) {
            refusals.push(CursorResetRefusal::WouldDuplicate(NonConvergentTable {
                table: name.to_string(),
                reason,
            }));
        }
    }
    refusals
}

#[cfg(test)]
mod tests {
    use airway::schema::{Column, Table};
    use airway::types::DataType;

    use super::*;

    fn table(name: &str, disposition: WriteDisposition) -> Table {
        let mut t = Table::new(name);
        t.write_disposition = disposition;
        t
    }

    fn keyed(name: &str, disposition: WriteDisposition, pk: &str) -> Table {
        let mut t = table(name, disposition);
        let mut col = Column::new(pk, DataType::Text);
        col.primary_key = true;
        t.columns.insert(pk.to_string(), col);
        t
    }

    fn schema_of(tables: Vec<Table>) -> Schema {
        let mut s = Schema::new("test");
        for t in tables {
            s.tables.insert(t.name.clone(), t);
        }
        s
    }

    #[test]
    fn append_never_converges() {
        assert_eq!(
            table_converges_on_repull(&table("vendor_forecasting", WriteDisposition::Append)),
            Err(NonConvergentReason::AppendOnly)
        );
    }

    #[test]
    fn replace_always_converges() {
        assert!(table_converges_on_repull(&table("dim", WriteDisposition::Replace)).is_ok());
    }

    #[test]
    fn keyed_merge_converges() {
        assert!(
            table_converges_on_repull(&keyed("vendor_sales", WriteDisposition::Merge, "asin"))
                .is_ok()
        );
    }

    /// The case the disposition's name defeats: `upsert_impl` falls through to
    /// `load_impl` — a plain append — when the key set is empty.
    #[test]
    fn keyless_merge_is_an_append_in_disguise() {
        assert_eq!(
            table_converges_on_repull(&table("vendor_sales", WriteDisposition::Merge)),
            Err(NonConvergentReason::NoDedupKey(WriteDisposition::Merge))
        );
    }

    #[test]
    fn keyless_merge_refusal_names_the_disposition() {
        let msg = NonConvergentTable {
            table: "vendor_sales".into(),
            reason: NonConvergentReason::NoDedupKey(WriteDisposition::Merge),
        }
        .to_string();
        assert!(
            msg.contains("`merge` with no effective key, which appends"),
            "{msg}"
        );
    }

    #[test]
    fn replace_by_parent_accepts_a_merge_key() {
        let mut t = table("orders__checks", WriteDisposition::ReplaceByParent);
        let mut col = Column::new("order_guid", DataType::Text);
        col.merge_key = true;
        t.columns.insert("order_guid".to_string(), col);
        assert!(table_converges_on_repull(&t).is_ok());
    }

    /// BMG's `amazon_vc`: flat resources, one table each. Resetting
    /// `vendor_sales` must not be blocked by `vendor_forecasting`.
    #[test]
    fn sibling_append_table_does_not_block_a_scoped_reset() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["vendor_sales".to_string(), "vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            &held,
            &[],
        );
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    #[test]
    fn resetting_every_cursor_is_blocked_by_the_append_table() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["vendor_sales".to_string(), "vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(Some(&schema), &CursorScope::AllResources, &held, &[]);
        assert_eq!(refusals.len(), 1, "{refusals:?}");
        assert!(refusals[0].to_string().contains("vendor_forecasting"));
    }

    /// The conservatism the unpopulated `Table::parent` forces: a renamed child
    /// belongs to *someone*, and the schema cannot say who.
    #[test]
    fn an_unclaimed_table_widens_the_judgement_and_names_itself() {
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            keyed("payments", WriteDisposition::Merge, "guid"),
            // Toast renames `orders__checks` to this, so no root claims it.
            table("order_checks", WriteDisposition::Append),
        ]);
        // Two held, one named: a genuinely narrower scope.
        let held = vec!["orders".to_string(), "payments".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["orders".into()]),
            &held,
            &[],
        );
        assert!(
            refusals.contains(&CursorResetRefusal::UnknownOwnership {
                table: "order_checks".into()
            }),
            "the refusal must name the unclaimed table: {refusals:?}"
        );
        // And the widening must have teeth — `order_checks` is judged, not
        // merely mentioned.
        assert!(
            refusals.iter().any(|r| matches!(
                r,
                CursorResetRefusal::WouldDuplicate(t) if t.table == "order_checks"
            )),
            "widening must actually judge the unclaimed table: {refusals:?}"
        );
    }

    /// Nested children the normalizer named are attributed, so they neither
    /// widen the judgement nor escape it.
    #[test]
    fn a_prefix_named_child_is_owned_by_its_root() {
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            table("orders__checks", WriteDisposition::Append),
        ]);
        let held = vec!["orders".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["orders".into()]),
            &held,
            &[],
        );
        assert!(
            refusals
                .iter()
                .all(|r| !matches!(r, CursorResetRefusal::UnknownOwnership { .. })),
            "a `<root>__<child>` table is claimed, not unknown: {refusals:?}"
        );
        assert_eq!(refusals.len(), 1, "but it is still judged: {refusals:?}");
    }

    /// Resetting every cursor re-pulls every resource, so a table no root
    /// claims is re-written too and must be judged. Narrowing to claimed
    /// tables here would let precisely the table nothing can account for be
    /// the one that escapes — the opposite of the conservatism that governs
    /// the scoped path.
    #[test]
    fn resetting_every_cursor_judges_an_unclaimed_table_too() {
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            // Renamed by the connector, so no resource root claims it.
            table("order_checks", WriteDisposition::Append),
        ]);
        let held = vec!["orders".to_string()];
        let refusals = cursor_reset_refusals(Some(&schema), &CursorScope::AllResources, &held, &[]);
        assert!(
            refusals.iter().any(|r| matches!(
                r,
                CursorResetRefusal::WouldDuplicate(t) if t.table == "order_checks"
            )),
            "an unclaimed append table must not escape a pipeline-wide reset: {refusals:?}"
        );
        // …and nothing is reported as unknown-ownership: no scope was narrowed.
        assert!(
            refusals
                .iter()
                .all(|r| !matches!(r, CursorResetRefusal::UnknownOwnership { .. })),
            "AllResources narrows nothing, so it explains nothing: {refusals:?}"
        );
    }

    /// Repeating a scoped reset that just succeeded. `vendor_sales` no longer
    /// holds a cursor, and the spec omits `resources:` — the case where only
    /// the scope's own names can say who owns `vendor_sales`. Attributing
    /// against `held` alone made this widen to the whole schema and refuse on
    /// `vendor_forecasting`, a table the caller never named.
    #[test]
    fn repeating_a_scoped_reset_does_not_widen_once_the_cursor_is_gone() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            &held,
            &[],
        );
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// The repeat above now clears nothing and short-circuits, so it no longer
    /// shows why the scope's own names are in the attribution universe. This
    /// does: `vendor_sales` was cleared, `payments` still holds a cursor, and
    /// the scope names both. Without the scope term `vendor_sales` claims
    /// nothing, the reset widens, and it is refused for scoping.
    #[test]
    fn a_named_resource_that_holds_no_cursor_still_claims_its_table() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            keyed("payments", WriteDisposition::Merge, "guid"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["payments".to_string(), "vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["vendor_sales".into(), "payments".into()]),
            &held,
            &[],
        );
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// A declared full-refresh resource never holds a cursor, so `held` never
    /// names it — but it still owns its table. Without `declared` in the
    /// universe, every scoped reset on such a pipeline widened and refused.
    #[test]
    fn a_declared_resource_without_a_cursor_still_claims_its_table() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["vendor_sales".to_string()];
        let declared = vec!["vendor_sales".to_string(), "vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["vendor_sales".into()]),
            &held,
            &declared,
        );
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// The two refusals are different claims, and the code a client
    /// branches on has to say which. An unscopeable reset asserts nothing
    /// about duplication; labelling it `would_duplicate` sent a client that
    /// auto-decides on the code the wrong way.
    #[test]
    fn each_refusal_carries_its_own_kind() {
        let duplicate = CursorResetRefusal::WouldDuplicate(NonConvergentTable {
            table: "vendor_forecasting".into(),
            reason: NonConvergentReason::AppendOnly,
        });
        let unknown = CursorResetRefusal::UnknownOwnership {
            table: "order_checks".into(),
        };
        assert_eq!(
            (duplicate.kind(), duplicate.table()),
            ("would_duplicate", "vendor_forecasting")
        );
        assert_eq!(
            (unknown.kind(), unknown.table()),
            ("unknown_ownership", "order_checks")
        );
    }

    /// Pins the assumption `resource_owns` makes (see its docs): tables oxy
    /// lands are named under `SnakeCase`. Two halves, because either could
    /// move — airway could change its default, or oxy could start choosing
    /// a convention. Neither is reachable today; this turns "not reachable
    /// today" into a decision the next person has to make on purpose.
    #[test]
    fn the_naming_this_assumes_is_the_one_oxy_lands_with() {
        let pipeline = airway::Pipeline::new("p", airway::MemoryDestination::new("d"));
        assert!(
            matches!(
                pipeline.normalizer.naming,
                airway::normalizer::NamingConvention::SnakeCase
            ),
            "airway's default normalizer is no longer SnakeCase; `resource_owns` \
             must use whatever convention landed the tables"
        );

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut dirs = vec![src];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("reset/convergence.rs")
                {
                    // Code lines only. A comment saying we deliberately never
                    // call `with_normalizer` is exactly the documentation this
                    // pin should encourage, not fail on.
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (i, line) in text.lines().enumerate() {
                        let code = line.trim_start();
                        if code.starts_with("//") {
                            continue;
                        }
                        if code.contains(".with_normalizer(")
                            || code.contains("NamingConvention::Direct")
                        {
                            offenders.push(format!("{}:{}: {code}", path.display(), i + 1));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "oxy now configures airway's naming convention (code, not comments) at \
             {offenders:?}; `resource_owns` hard-codes SnakeCase and must follow it"
        );
    }

    /// A whole-pipeline rewind sent as the explicit list. On a connector that
    /// renames its nested children, the list used to draw `UnknownOwnership`
    /// on every such rewind — routine enough to train `force`. Covering every
    /// held cursor narrows nothing, so it has nothing to explain.
    #[test]
    fn a_list_naming_every_held_resource_is_not_refused_for_scoping() {
        let mut checks = table("order_checks", WriteDisposition::ReplaceByParent);
        let mut col = Column::new("order_guid", DataType::Text);
        col.merge_key = true;
        checks.columns.insert("order_guid".to_string(), col);
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            checks,
        ]);
        let held = vec!["orders".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["orders".into()]),
            &held,
            &[],
        );
        assert!(refusals.is_empty(), "{refusals:?}");
    }

    /// …but it is judged as the whole pipeline it is, so an unclaimed table
    /// that appends still refuses it — exactly as `AllResources` would.
    #[test]
    fn a_list_naming_every_held_resource_still_judges_every_table() {
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            table("order_checks", WriteDisposition::Append),
        ]);
        let held = vec!["orders".to_string()];
        let covering = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["orders".into()]),
            &held,
            &[],
        );
        let all = cursor_reset_refusals(Some(&schema), &CursorScope::AllResources, &held, &[]);
        assert_eq!(covering, all);
        assert!(
            covering.iter().any(|r| matches!(
                r,
                CursorResetRefusal::WouldDuplicate(t) if t.table == "order_checks"
            )),
            "{covering:?}"
        );
    }

    /// An unclaimed table widens the judgement to what the reset may re-pull
    /// — not to a sibling's tables. `vendor_forecasting` is claimed by a
    /// resource outside the scope; its cursor is untouched, so it must not be
    /// blamed. The review's failure shape was a refusal naming exactly that.
    #[test]
    fn widening_over_an_unclaimed_table_does_not_blame_a_sibling() {
        let mut checks = table("order_checks", WriteDisposition::ReplaceByParent);
        let mut col = Column::new("order_guid", DataType::Text);
        col.merge_key = true;
        checks.columns.insert("order_guid".to_string(), col);
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            checks,
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["orders".to_string(), "vendor_forecasting".to_string()];
        let refusals = cursor_reset_refusals(
            Some(&schema),
            &CursorScope::Resources(vec!["orders".into()]),
            &held,
            &[],
        );
        assert_eq!(
            refusals,
            vec![CursorResetRefusal::UnknownOwnership {
                table: "order_checks".into()
            }]
        );
    }

    /// A whole-pipeline reset clears every *held* cursor. A declared
    /// full-refresh resource holds none, so nothing about it changes and its
    /// append table is not re-pulled by this reset.
    #[test]
    fn a_whole_pipeline_reset_does_not_judge_a_resource_it_clears_nothing_for() {
        let schema = schema_of(vec![
            keyed("vendor_sales", WriteDisposition::Merge, "asin"),
            table("vendor_forecasting", WriteDisposition::Append),
        ]);
        let held = vec!["vendor_sales".to_string()];
        let declared = vec!["vendor_sales".to_string(), "vendor_forecasting".to_string()];
        for scope in [
            CursorScope::AllResources,
            CursorScope::Resources(vec!["vendor_sales".into()]),
        ] {
            let refusals = cursor_reset_refusals(Some(&schema), &scope, &held, &declared);
            assert!(refusals.is_empty(), "{scope:?}: {refusals:?}");
        }
    }

    /// A reset whose scope names no cursor this pipeline holds clears
    /// nothing, so it re-pulls nothing and cannot duplicate anything. Judged
    /// as a narrowing, it was refused — the retry of a whole-pipeline rewind
    /// that had just succeeded, from `oxyc` or any retry loop, is exactly this
    /// request, and the same "refuses its own successful request" shape the
    /// attribution fix exists to end.
    #[test]
    fn a_reset_that_clears_nothing_refuses_nothing() {
        let schema = schema_of(vec![
            keyed("orders", WriteDisposition::Merge, "guid"),
            // Renamed by the connector, so no resource root claims it.
            table("order_checks", WriteDisposition::Append),
        ]);
        for scope in [
            CursorScope::Resources(vec!["orders".into()]),
            CursorScope::AllResources,
        ] {
            let refusals = cursor_reset_refusals(Some(&schema), &scope, &[], &[]);
            assert!(refusals.is_empty(), "{scope:?}: {refusals:?}");
        }
    }

    #[test]
    fn no_stored_schema_has_nothing_to_duplicate() {
        assert!(cursor_reset_refusals(None, &CursorScope::AllResources, &[], &[]).is_empty());
    }
}
