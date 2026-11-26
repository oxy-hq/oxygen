//! The capture functions' bodies: PL/pgSQL, one per operation, each baked
//! with its table's oid, key, captured columns and schema version (OLTP
//! change stream design §5.2). [`super::install`] wraps them in `CREATE
//! FUNCTION`.
//!
//! Each body, in order:
//! 1. refuses to run for any table but its own (`TG_RELID`), so an app cannot
//!    attach it elsewhere to write history in this table's name;
//! 2. returns at once when its transition table is empty;
//! 3. reads the store's stream: no row fails the write, since an unseeded
//!    stream must fail loudly rather than go uncaptured, and `capturing`
//!    false writes nothing;
//! 4. reads who made the change from the three transaction-local settings
//!    the host sets, where an empty or malformed value is null and never
//!    fails the write (the relay verifies the MAC, not this);
//! 5. writes the log rows in one `INSERT … SELECT` from the transition table.
//!
//! **No column is referenced by name.** An image is the row as `jsonb`,
//! filtered to the captured columns' names; a key is the same over the
//! primary key's. So a migration that adds, drops or renames a column of a
//! captured table and then writes to it in the same file still succeeds: a
//! dropped column stops appearing, and an added one is not captured until
//! the next install records a new shape. An update pairs old and new rows on
//! that key `jsonb` too, which is why the pairing holds through a renamed
//! key column, and why no name the app chose reaches this SQL as an
//! identifier. `image(r.*)`, not `image(r)`: a bare `r` is a column when the
//! table has one named `r`.
//!
//! **The row becomes `jsonb` through the writer's `oxy_changes.image_<writer>`,
//! never `to_jsonb` here.** `to_jsonb` runs a cast to `json` an app defined on its own type;
//! called from this definer it would run as the store's owner. The image
//! function is a definer owned by a role with no privileges, so the cast runs
//! as that role ([`super::image`]).
//!
//! `#variable_conflict use_variable`, because the statements read the
//! function's own variables beside a transition table whose columns the app
//! chose: an app column named `v_actor` must not make every write ambiguous.

use crate::platform::{
    CAPTURE_FUNCTION_PREFIX, CHANGES_ACTOR_SETTING, CHANGES_INVOCATION_SETTING, CHANGES_LOG_TABLE,
    CHANGES_PROVENANCE_SETTING, CHANGES_STREAM_TABLE,
};
use crate::roles::quote_literal;

use super::CaptureSpec;
use super::image::image_function;

/// The four operations a table's capture covers, one function and one
/// trigger each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Op {
    Insert,
    Update,
    Delete,
    Truncate,
}

impl Op {
    pub(super) const ALL: [Op; 4] = [Op::Insert, Op::Update, Op::Delete, Op::Truncate];

    /// The log's `op` for it, and the suffix of its function's name.
    pub(super) fn code(self) -> &'static str {
        match self {
            Op::Insert => "i",
            Op::Update => "u",
            Op::Delete => "d",
            Op::Truncate => "t",
        }
    }
}

/// `_oxy_capture_<relid>_<op>`: unquoted, and needing no quotes.
pub(super) fn function_name(relid: u32, op: Op) -> String {
    format!("{CAPTURE_FUNCTION_PREFIX}{relid}_{}", op.code())
}

/// Return before reading the stream when the statement changed no row: an
/// `INSERT … ON CONFLICT DO UPDATE` fires both statement triggers whatever it
/// did.
const EMPTY_NEW: &str =
    "  IF NOT EXISTS (SELECT 1 FROM oxy_new) THEN\n    RETURN NULL;\n  END IF;\n";
const EMPTY_OLD: &str =
    "  IF NOT EXISTS (SELECT 1 FROM oxy_old) THEN\n    RETURN NULL;\n  END IF;\n";

/// The body of `spec`'s capture function for `op`, as the module docs lay it
/// out. Not yet quoted: [`super::install`] picks a dollar-quote tag the body
/// does not contain.
pub(super) fn body(spec: &CaptureSpec<'_>, op: Op) -> String {
    let relid = spec.capture.shape.relid;
    let name = function_name(relid, op);
    let early = match op {
        Op::Insert | Op::Update => EMPTY_NEW,
        Op::Delete => EMPTY_OLD,
        Op::Truncate => "",
    };
    let insert = log_insert(spec, op);
    format!(
        "
#variable_conflict use_variable
DECLARE
  v_generation bigint;
  v_capturing boolean;
  v_actor text;
  v_invocation text;
  v_provenance text;
BEGIN
  IF TG_RELID <> {relid}::oid THEN
    RAISE EXCEPTION 'oxy_changes: {name} captures only the table it was generated for, not %', TG_RELID::regclass
      USING ERRCODE = 'insufficient_privilege';
  END IF;
{early}  SELECT s.generation, s.capturing INTO v_generation, v_capturing FROM {CHANGES_STREAM_TABLE} s;
  IF NOT FOUND THEN
    RAISE EXCEPTION 'oxy_changes: {CHANGES_STREAM_TABLE} has no row, so this store''s change stream was never seeded and the write cannot be captured'
      USING ERRCODE = 'object_not_in_prerequisite_state';
  END IF;
  IF NOT v_capturing THEN
    RETURN NULL;
  END IF;
  v_actor := NULLIF(current_setting('{CHANGES_ACTOR_SETTING}', true), '');
  v_invocation := NULLIF(current_setting('{CHANGES_INVOCATION_SETTING}', true), '');
  IF v_invocation !~ '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$' THEN
    v_invocation := NULL;
  END IF;
  v_provenance := NULLIF(current_setting('{CHANGES_PROVENANCE_SETTING}', true), '');
  IF v_provenance !~ '^[0-9a-f]{{64}}$' THEN
    v_provenance := NULL;
  END IF;
  {insert};
  RETURN NULL;
END
"
    )
}

/// The one `INSERT … SELECT` writing the statement's log rows.
fn log_insert(spec: &CaptureSpec<'_>, op: Op) -> String {
    let shape = &spec.capture.shape;
    let columns = text_array(spec.capture.captured.iter().map(|c| c.name.as_str()));
    let key = text_array(shape.primary_key.iter().map(String::as_str));
    let head = format!(
        "INSERT INTO {CHANGES_LOG_TABLE} (generation, writer, table_name, schema_version, op, key, \
         row_image, actor, invocation, provenance)"
    );
    let stamp = format!(
        "v_generation, {}, {}, {}",
        quote_literal(spec.writer),
        quote_literal(&shape.table),
        spec.schema_version
    );
    let signed = "v_actor, v_invocation::uuid, decode(v_provenance, 'hex')";
    let image = image_function(spec.writer);
    match op {
        Op::Insert => format!(
            "{head}\n  SELECT {stamp}, 'i'::\"char\", {}, {}, {signed}\n    FROM oxy_new r",
            filtered(&image, "r", &key),
            filtered(&image, "r", &columns)
        ),
        Op::Delete => format!(
            "{head}\n  SELECT {stamp}, 'd'::\"char\", {}, {}, {signed}\n    FROM oxy_old r",
            filtered(&image, "r", &key),
            filtered(&image, "r", &columns)
        ),
        Op::Update => {
            let side = |table: &str| {
                format!(
                    "(SELECT true AS present, {} AS k, {} AS img FROM {table} r)",
                    filtered(&image, "r", &key),
                    filtered(&image, "r", &columns)
                )
            };
            format!(
                "{head}
  SELECT {stamp},
         CASE WHEN o.present IS NULL THEN 'i' WHEN n.present IS NULL THEN 'd' ELSE 'u' END::\"char\",
         CASE WHEN n.present IS NULL THEN o.k ELSE n.k END,
         CASE WHEN n.present IS NULL THEN o.img ELSE n.img END,
         {signed}
    FROM {old} o
    FULL OUTER JOIN {new} n ON o.k = n.k
   WHERE o.present IS NULL OR n.present IS NULL OR o.img IS DISTINCT FROM n.img",
                old = side("oxy_old"),
                new = side("oxy_new"),
            )
        }
        Op::Truncate => {
            format!("{head}\n  VALUES ({stamp}, 't'::\"char\", NULL, NULL, {signed})")
        }
    }
}

/// Row `alias` as `jsonb` through `image`, holding only the keys `names` (a
/// `text[]`) lists. `image` is the function that turns the row into `jsonb`,
/// and it runs any cast to `json` an app defined: SQL running as a definer
/// passes its writer's [`image_function`], so the cast runs with no
/// privileges.
/// Only SQL running in the writer's own session may pass
/// `pg_catalog.to_jsonb`, since a cast there runs as the writer anyway.
fn filtered(image: &str, alias: &str, names: &str) -> String {
    format!(
        "(SELECT jsonb_object_agg(e.key, e.value) FROM jsonb_each({image}({alias}.*)) e \
         WHERE e.key = ANY({names}))"
    )
}

/// `ARRAY['a','b']::text[]`: column names as literals, never identifiers.
fn text_array<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = names.map(quote_literal).collect();
    format!("ARRAY[{}]::text[]", quoted.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::shape::leases;

    fn bodies() -> Vec<(Op, String)> {
        let capture = leases();
        let spec = CaptureSpec {
            writer: "x",
            capture: &capture,
            schema_version: 1,
        };
        Op::ALL.iter().map(|&op| (op, body(&spec, op))).collect()
    }

    fn body_of(op: Op) -> String {
        bodies()
            .into_iter()
            .find(|(o, _)| *o == op)
            .map(|(_, b)| b)
            .expect("a body per op")
    }

    #[test]
    fn function_refuses_another_table() {
        for (op, body) in bodies() {
            assert!(
                body.contains("IF TG_RELID <> 16423::oid THEN"),
                "{op:?}: {body}"
            );
            assert!(
                body.contains(&format!(
                    "'oxy_changes: _oxy_capture_16423_{} captures only the table it was \
                     generated for, not %'",
                    op.code()
                )),
                "{op:?}: {body}"
            );
            let refusal = body.find("TG_RELID <>").expect("the check");
            let stream = body
                .find("FROM oxy_changes.stream")
                .expect("the stream read");
            assert!(refusal < stream, "refused before anything is read: {body}");
        }
    }

    #[test]
    fn image_sql_filters_by_name_not_by_reference() {
        let insert = body_of(Op::Insert);
        assert!(
            insert.contains("jsonb_each(oxy_changes.\"image_x\"(r.*))"),
            "{insert}"
        );
        assert!(
            insert.contains("WHERE e.key = ANY(ARRAY['id','name']::text[])"),
            "the image keeps the captured columns: {insert}"
        );
        assert!(
            insert.contains("WHERE e.key = ANY(ARRAY['id']::text[])"),
            "the key keeps the primary key's: {insert}"
        );
        for (op, body) in bodies() {
            assert!(!body.contains("oxy_new.\""), "{op:?}: {body}");
            assert!(!body.contains("oxy_old.\""), "{op:?}: {body}");
            assert!(!body.contains("r.\""), "{op:?}: {body}");
            assert!(
                !body.contains("contact_email"),
                "an excluded column is named nowhere: {op:?}: {body}"
            );
        }
    }

    /// An `ON CONFLICT DO UPDATE` that changes nothing fires the update
    /// trigger with equal images, and writes no row.
    #[test]
    fn update_drops_unchanged_pairs() {
        let update = body_of(Op::Update);
        assert!(update.contains("FULL OUTER JOIN"), "{update}");
        assert!(
            update.contains("ON o.k = n.k"),
            "paired on the key: {update}"
        );
        assert!(
            update.contains(
                "WHERE o.present IS NULL OR n.present IS NULL OR o.img IS DISTINCT FROM n.img"
            ),
            "{update}"
        );
        assert!(
            update.contains(
                "CASE WHEN o.present IS NULL THEN 'i' WHEN n.present IS NULL THEN 'd' ELSE 'u' END"
            ),
            "a key change is a delete and an insert: {update}"
        );
        assert!(update.contains("FROM oxy_old r) o"), "{update}");
        assert!(update.contains("FROM oxy_new r) n"), "{update}");
    }

    #[test]
    fn each_op_reads_its_own_transition_table() {
        let insert = body_of(Op::Insert);
        assert!(insert.contains("'i'::\"char\""), "{insert}");
        assert!(insert.contains("FROM oxy_new r"), "{insert}");
        assert!(!insert.contains("oxy_old"), "{insert}");
        let delete = body_of(Op::Delete);
        assert!(delete.contains("'d'::\"char\""), "{delete}");
        assert!(
            delete.contains("FROM oxy_old r"),
            "the before-image: {delete}"
        );
        assert!(!delete.contains("oxy_new"), "{delete}");
        let truncate = body_of(Op::Truncate);
        assert!(
            truncate.contains("VALUES (v_generation, 'x', 'leases', 1, 't'::\"char\", NULL, NULL,"),
            "one marker, no key or image: {truncate}"
        );
        assert!(!truncate.contains("oxy_new") && !truncate.contains("oxy_old"));
    }

    /// No stream row fails the write; a stream not capturing writes nothing;
    /// a malformed setting is null rather than an error.
    #[test]
    fn the_stream_and_the_settings_are_read_as_the_design_says() {
        let body = body_of(Op::Insert);
        assert!(
            body.starts_with("\n#variable_conflict use_variable\nDECLARE"),
            "{body}"
        );
        assert!(
            body.contains(
                "IF NOT FOUND THEN\n    RAISE EXCEPTION 'oxy_changes: oxy_changes.stream has no row"
            ),
            "{body}"
        );
        assert!(
            body.contains("IF NOT v_capturing THEN\n    RETURN NULL;"),
            "{body}"
        );
        for setting in [
            CHANGES_ACTOR_SETTING,
            CHANGES_INVOCATION_SETTING,
            CHANGES_PROVENANCE_SETTING,
        ] {
            assert!(
                body.contains(&format!("NULLIF(current_setting('{setting}', true), '')")),
                "{setting}: {body}"
            );
        }
        assert!(body.contains("v_provenance !~ '^[0-9a-f]{64}$'"), "{body}");
        assert!(
            body.contains(
                "v_invocation !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'"
            ),
            "only a value the uuid cast accepts survives: {body}"
        );
        assert!(
            body.contains("v_actor, v_invocation::uuid, decode(v_provenance, 'hex')"),
            "{body}"
        );
        let early = body
            .find("IF NOT EXISTS (SELECT 1 FROM oxy_new)")
            .expect("early return");
        assert!(
            early < body.find("FROM oxy_changes.stream").unwrap(),
            "{body}"
        );
    }

    /// A capture function never calls `to_jsonb` itself, which would run an
    /// app's cast as the store's owner: every key and image goes through the
    /// image function, the key and the image of each side.
    #[test]
    fn every_image_goes_through_the_image_role() {
        for (op, body) in bodies() {
            assert!(!body.contains("to_jsonb"), "{op:?}: {body}");
            assert!(!body.contains("to_json("), "{op:?}: {body}");
            let images = body.matches("oxy_changes.\"image_x\"(r.*)").count();
            let expected = match op {
                Op::Insert | Op::Delete => 2,
                Op::Update => 4,
                Op::Truncate => 0,
            };
            assert_eq!(images, expected, "{op:?}: {body}");
        }
    }

    #[test]
    fn a_quote_in_a_column_name_stays_a_literal() {
        let mut capture = leases();
        capture.captured[1].name = "o'brien".into();
        let spec = CaptureSpec {
            writer: "x",
            capture: &capture,
            schema_version: 2,
        };
        let body = body(&spec, Op::Insert);
        assert!(body.contains("ARRAY['id','o''brien']::text[]"), "{body}");
        assert!(body.contains("'x', 'leases', 2,"), "{body}");
    }
}
