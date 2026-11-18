//! An event's JSON Schema (draft 2020-12) as the platform keeps it: a build's
//! declared schema becomes an immutable revision at publish, a later change
//! is held against the old one for compatibility, and every payload is
//! validated against the revision the publishing build runs.
//!
//! No remote `$ref` is ever resolved: `jsonschema` is built without its
//! `resolve-http` / `resolve-file` features, so a schema that points outside
//! itself is refused rather than fetched.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use chrono::Utc;
use entity::{app_builds, app_environments, app_event_schemas};
use oxy_custom_apps::events::catalog::EventCatalog;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A schema must be a valid draft 2020-12 schema describing an object: an
/// event's payload is a JSON object, so any other root type could never match.
pub fn check_schema(schema: &Value) -> Result<(), String> {
    if !schema.is_object() {
        return Err("the schema must be a JSON object".into());
    }
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err("the schema's root `type` must be \"object\"".into());
    }
    jsonschema::draft202012::meta::validate(schema)
        .map_err(|e| format!("not a valid JSON Schema (draft 2020-12): {e}"))?;
    // Compiling resolves `$ref`s; with remote resolution off, an external one fails here.
    jsonschema::draft202012::new(schema)
        .map(|_| ())
        .map_err(|e| format!("not a valid JSON Schema (draft 2020-12): {e}"))
}

/// What a move from `old` to `new` breaks for a publisher written against
/// `old`; empty means compatible. Each finding names its path with dots,
/// `[]` for an array's elements and `[i]` for a tuple slot (`lines[].amount`,
/// `pair[1]`):
///
/// - a property removed, retyped, or made required (an added optional one is
///   compatible), at any depth of `properties`;
/// - the same inside an array's `items` and each `prefixItems` slot; a slot
///   `new` adds is held against what `old` allowed there (its `items`, or
///   anything when it has none; nothing at all after a closed tuple); a slot
///   `new` removes is held against new's `items` (removed, after a closed
///   tuple); `items` that `new` adds where `old` had none narrow from
///   anything; and a tuple `new` closes (`items: false`) where `old` was
///   open removes the elements past it;
/// - behind a local `$ref` (`#/$defs/Money`, `#`), followed in its own
///   schema, so a change to a shared definition is found; keywords beside a
///   `$ref` count with its target, as `allOf: [target, siblings]` would;
/// - across `allOf`, whose members' properties and `required` count as the
///   schema's own.
///
/// **Bounded, whatever the schemas.** Each pair of schemas is compared once:
/// a `$ref` reached again — a recursive schema, or a definition used in many
/// places — is not walked again, so a change inside it is reported once,
/// where it is first reached. Each schema's view gathers each node once.
/// Past [`MAX_COMPARED_NODES`] or [`MAX_WALK_DEPTH`] the comparison stops and
/// **fails closed**: [`TOO_COMPLEX`] is added, once, to what it found, and is
/// refused (or overridden with `--break-listeners`) like any other finding.
/// Remote refs never get here: [`check_schema`] refuses them.
pub fn incompatibilities(old: &Value, new: &Value) -> Vec<String> {
    let mut walker = Walker::new(old, new);
    walker.walk(old, new, "", 0);
    walker.finish()
}

/// How deep a walk goes — pairs of schemas inside each other, or nodes a
/// view reaches through `$ref` and `allOf` — before it fails closed. Far past
/// any real event's nesting; it keeps the recursion off the stack's limit.
const MAX_WALK_DEPTH: usize = 64;

/// How much one comparison reads before it fails closed: each pair of schemas
/// compared, each node a view reaches through `$ref` or `allOf`, each
/// property, `required` name and tuple slot a view copies, and each property
/// and slot a pair compares. A real event's schema reads a few hundred; an
/// honest one at the 16 KiB cap, built by inheritance (`allOf` over a shared
/// base), around ten thousand — so the budget sits ten times above that, and
/// only amplification (a wide definition copied into many views) reaches it,
/// which it does within milliseconds.
const MAX_COMPARED_NODES: usize = 100_000;

/// The one finding of a comparison that stopped before it read everything.
pub const TOO_COMPLEX: &str = "schema too complex to compare";

/// What a tuple slot holds when nothing constrains it: the schema `true`.
static ANYTHING: Value = Value::Bool(true);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Old,
    New,
}

/// A node's identity, by address.
fn address(node: &Value) -> usize {
    std::ptr::from_ref(node) as usize
}

struct Walker<'a> {
    old_root: &'a Value,
    new_root: &'a Value,
    /// The (old, new) pairs compared or being compared, by [`Walker::identity`].
    compared: HashSet<(usize, usize)>,
    /// Each node's view, gathered once, by side and [`Walker::identity`].
    views: HashMap<(Side, usize), Rc<View<'a>>>,
    /// Nodes left to read ([`MAX_COMPARED_NODES`]).
    budget: usize,
    /// The budget or the depth ran out: the comparison fails closed.
    exhausted: bool,
    found: Vec<String>,
}

impl<'a> Walker<'a> {
    fn new(old_root: &'a Value, new_root: &'a Value) -> Self {
        Self {
            old_root,
            new_root,
            compared: HashSet::new(),
            views: HashMap::new(),
            budget: MAX_COMPARED_NODES,
            exhausted: false,
            found: Vec::new(),
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.exhausted {
            self.found.push(TOO_COMPLEX.to_string());
        }
        self.found
    }

    fn root(&self, side: Side) -> &'a Value {
        match side {
            Side::Old => self.old_root,
            Side::New => self.new_root,
        }
    }

    /// Read one more node, at `depth`; `false`, and the comparison failing
    /// closed, once the budget or the depth is spent.
    fn read(&mut self, depth: usize) -> bool {
        if depth >= MAX_WALK_DEPTH {
            self.exhausted = true;
            return false;
        }
        self.spend()
    }

    /// Spend one unit of the budget on one piece of work — a property, a
    /// `required` name or a tuple slot copied into a view, a property or slot
    /// a pair compares; `false`, and the comparison failing closed, once it
    /// is spent. So the whole comparison's work, not just the nodes it
    /// visits, stays within [`MAX_COMPARED_NODES`].
    fn spend(&mut self) -> bool {
        if self.exhausted || self.budget == 0 {
            self.exhausted = true;
            return false;
        }
        self.budget -= 1;
        true
    }

    /// `node`'s identity: a `$ref` with nothing beside it is the schema it
    /// names, so every use of a definition is one schema; a `$ref` with
    /// keywords beside it is a schema of its own.
    fn identity(&self, mut node: &'a Value, side: Side) -> usize {
        let root = self.root(side);
        for _ in 0..MAX_WALK_DEPTH {
            let bare = node.as_object().is_some_and(|keywords| keywords.len() == 1);
            match local_ref(node, root) {
                Some(target) if bare && !std::ptr::eq(target, node) => node = target,
                _ => break,
            }
        }
        address(node)
    }

    fn walk(&mut self, old: &'a Value, new: &'a Value, prefix: &str, depth: usize) {
        let pair = (self.identity(old, Side::Old), self.identity(new, Side::New));
        // Compared already, or being compared further up (a cycle).
        if self.exhausted || !self.compared.insert(pair) || !self.read(depth) {
            return;
        }
        let (o, n) = (self.view(old, Side::Old), self.view(new, Side::New));
        let path = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            }
        };

        for &(name, old_prop) in &o.properties {
            if !self.spend() {
                return;
            }
            match n.property(name) {
                None => self.found.push(format!("`{}` was removed", path(name))),
                Some(new_prop) => {
                    self.retyped(old_prop, new_prop, &path(name));
                    if n.required.contains(name) && !o.required.contains(name) {
                        self.found.push(format!("`{}` became required", path(name)));
                    }
                    self.walk(old_prop, new_prop, &path(name), depth + 1);
                }
            }
        }
        // A new property that is required rejects every publisher that omits it.
        for &(name, _) in &n.properties {
            if !self.spend() {
                return;
            }
            if o.property(name).is_none() && n.required.contains(name) {
                self.found.push(format!("`{}` became required", path(name)));
            }
        }
        let at = format!("{prefix}[]");
        match (o.items, n.items) {
            (Some(old_items), Some(new_items)) => {
                self.retyped(old_items, new_items, &at);
                self.walk(old_items, new_items, &at, depth + 1);
            }
            // `new` constrains elements old publishers were free to send: a
            // narrowing from anything. After a closed tuple they sent none.
            (None, Some(new_items)) if !o.closed => {
                self.retyped(&ANYTHING, new_items, &at);
                self.walk(&ANYTHING, new_items, &at, depth + 1);
            }
            // `new` closes the tuple: the elements old publishers were free to
            // send past it (under old's `items`, or anything) are refused.
            (_, None) if n.closed && !o.closed => {
                self.found.push(format!("`{at}` was removed"));
            }
            _ => {}
        }
        for (i, &new_slot) in n.prefix_items.iter().enumerate() {
            if !self.spend() {
                return;
            }
            let old_slot = match o.prefix_items.get(i) {
                Some(&slot) => slot,
                // A closed tuple's publishers send nothing there.
                None if o.closed => continue,
                // A slot `new` adds: what `old` allowed there.
                None => o.items.unwrap_or(&ANYTHING),
            };
            let at = format!("{prefix}[{i}]");
            self.retyped(old_slot, new_slot, &at);
            self.walk(old_slot, new_slot, &at, depth + 1);
        }
        // A slot `new` removes falls under new's `items`: held against them,
        // refused outright after a closed tuple, free (a widening) with none.
        for (i, &old_slot) in o.prefix_items.iter().enumerate().skip(n.prefix_items.len()) {
            if !self.spend() {
                return;
            }
            let at = format!("{prefix}[{i}]");
            match n.items {
                Some(new_items) => {
                    self.retyped(old_slot, new_items, &at);
                    self.walk(old_slot, new_items, &at, depth + 1);
                }
                None if n.closed => self.found.push(format!("`{at}` was removed")),
                None => {}
            }
        }
    }

    /// `at` changed type between `old` and `new`, each read in its own schema.
    fn retyped(&mut self, old: &'a Value, new: &'a Value, at: &str) {
        let (from, to) = (
            self.view(old, Side::Old).type_name(),
            self.view(new, Side::New).type_name(),
        );
        if from != to {
            self.found
                .push(format!("`{at}` changed type from {from} to {to}"));
        }
    }

    /// `node` as validation sees it, gathered once.
    fn view(&mut self, node: &'a Value, side: Side) -> Rc<View<'a>> {
        let key = (side, self.identity(node, side));
        if let Some(view) = self.views.get(&key) {
            return view.clone();
        }
        let mut view = View::default();
        self.gather(&mut view, &mut HashSet::new(), node, side, 0);
        let view = Rc::new(view);
        self.views.insert(key, view.clone());
        view
    }

    /// Merge `node` into `view`: its local `$ref`'s target first, then its
    /// own keywords, then each `allOf` member. Each node once (`seen`); a
    /// node reached through a `$ref` or `allOf` is read from the budget, and
    /// so is every property, `required` name and tuple slot copied in — a
    /// view of a schema beside a `$ref` copies its target, so many such
    /// schemas would otherwise copy one wide definition many times over.
    fn gather(
        &mut self,
        view: &mut View<'a>,
        seen: &mut HashSet<usize>,
        node: &'a Value,
        side: Side,
        depth: usize,
    ) {
        if !seen.insert(address(node)) || (depth > 0 && !self.read(depth)) {
            return;
        }
        if let Some(target) = local_ref(node, self.root(side)) {
            self.gather(view, seen, target, side, depth + 1);
        }
        for (name, prop) in node
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if !self.spend() {
                return;
            }
            view.add_property(name, prop);
        }
        for name in required_of(node) {
            if !self.spend() {
                return;
            }
            view.required.insert(name);
        }
        if view.ty.is_none() {
            view.ty = declared_type(node);
        }
        match node.get("items") {
            Some(items) if items.is_object() && view.items.is_none() => view.items = Some(items),
            Some(Value::Bool(false)) => view.closed = true,
            _ => {}
        }
        if view.prefix_items.is_empty()
            && let Some(slots) = node.get("prefixItems").and_then(Value::as_array)
        {
            for slot in slots {
                if !self.spend() {
                    return;
                }
                view.prefix_items.push(slot);
            }
        }
        for member in node
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.gather(view, seen, member, side, depth + 1);
        }
    }
}

/// The schema a `$ref` in the same document names (`#`, `#/$defs/Money`),
/// or `None` when `node` has none, or one that is not local or leads nowhere.
fn local_ref<'a>(node: &'a Value, root: &'a Value) -> Option<&'a Value> {
    let pointer = node.get("$ref")?.as_str()?.strip_prefix('#')?;
    if pointer.is_empty() {
        Some(root)
    } else {
        root.pointer(pointer)
    }
}

/// One schema as validation sees it: its local `$ref` followed, the keywords
/// beside it, and every `allOf` member's properties, `required` and the rest
/// merged in.
#[derive(Default)]
struct View<'a> {
    /// In declaration order, the first declaration of a name winning.
    properties: Vec<(&'a str, &'a Value)>,
    by_name: HashMap<&'a str, &'a Value>,
    required: BTreeSet<&'a str>,
    ty: Option<String>,
    items: Option<&'a Value>,
    /// `items: false`: no element past `prefix_items`.
    closed: bool,
    prefix_items: Vec<&'a Value>,
}

impl<'a> View<'a> {
    fn add_property(&mut self, name: &'a str, prop: &'a Value) {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.by_name.entry(name) {
            slot.insert(prop);
            self.properties.push((name, prop));
        }
    }

    fn property(&self, name: &str) -> Option<&'a Value> {
        self.by_name.get(name).copied()
    }

    /// The type as [`declared_type`] writes it; `any` when unconstrained.
    fn type_name(&self) -> String {
        self.ty.clone().unwrap_or_else(|| "any".to_string())
    }
}

fn required_of(schema: &Value) -> BTreeSet<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// A schema's own `type`: `string`, or `integer|null` for a union (members
/// sorted, so order alone is not a change); `None` when it declares none.
fn declared_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(ts)) => {
            let mut names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
            names.sort_unstable();
            Some(names.join("|"))
        }
        _ => None,
    }
}

/// Validate a payload against a schema; every violation names where it is
/// and the keyword it failed (`/period fails \`type\``), **never the value**:
/// a payload is the app's data, and a refusal's text reaches logs, run
/// records and replies. Nor a key the payload chose: a path segment the
/// schema does not declare is `*` ([`shown_path`]). A missing required
/// property is named by its path (`/period fails \`required\``), from the
/// schema; the payload's own unexpected keys are not named.
pub fn validate(schema: &Value, data: &Value) -> Result<(), Vec<String>> {
    let validator =
        jsonschema::draft202012::new(schema).map_err(|e| vec![format!("invalid schema: {e}")])?;
    let declared = declared_names(schema);
    let errors: Vec<String> = validator
        .iter_errors(data)
        .map(|e| {
            let mut at = shown_path(&e.instance_path().to_string(), data, &declared);
            if let jsonschema::error::ValidationErrorKind::Required { property } = e.kind()
                && let Some(property) = property.as_str()
            {
                // A JSON pointer's escapes: `~` then `/`.
                at.push('/');
                at.push_str(&property.replace('~', "~0").replace('/', "~1"));
            }
            if at.is_empty() {
                at.push('/');
            }
            format!("{at} fails `{}`", e.kind().keyword())
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Every name the schema declares under a `properties` keyword, anywhere in
/// it (its `$defs` and `allOf` members included): the words a refusal's path
/// may show.
fn declared_names(schema: &Value) -> HashSet<&str> {
    fn collect<'a>(node: &'a Value, names: &mut HashSet<&'a str>) {
        match node {
            Value::Object(keywords) => {
                for (keyword, value) in keywords {
                    if keyword == "properties"
                        && let Some(properties) = value.as_object()
                    {
                        names.extend(properties.keys().map(String::as_str));
                    }
                    collect(value, names);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect(item, names)),
            _ => {}
        }
    }
    let mut names = HashSet::new();
    collect(schema, &mut names);
    names
}

/// `pointer`, a JSON pointer into `data`, with each segment the payload chose
/// written `*`: walking `data` along it, an index into an array stays, and so
/// does a key the schema declares ([`declared_names`]); any other key — one
/// under `additionalProperties` or `patternProperties`, an email or an id the
/// app keyed a map by — is the app's data, never shown. A segment that is a
/// number is judged by what it indexes, so a numeric key is still a key.
fn shown_path(pointer: &str, data: &Value, declared: &HashSet<&str>) -> String {
    let mut shown = String::new();
    let mut at = Some(data);
    for raw in pointer.split('/').skip(1) {
        let segment = raw.replace("~1", "/").replace("~0", "~");
        let (kept, next) = match at {
            Some(Value::Array(items)) => match segment.parse::<usize>() {
                Ok(index) => (true, items.get(index)),
                Err(_) => (false, None),
            },
            Some(Value::Object(members)) => {
                (declared.contains(segment.as_str()), members.get(&segment))
            }
            _ => (false, None),
        };
        shown.push('/');
        shown.push_str(if kept { raw } else { "*" });
        at = next;
    }
    shown
}

/// The schema with every object's keys in sorted order, so the same schema
/// hashes the same however its keys were ordered (Postgres `jsonb` reorders
/// them, and the manifest preserves the author's order).
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> =
                map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The hex SHA-256 of the schema's canonical JSON.
pub fn schema_sha256(schema: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical(schema)).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// The revision holding exactly this schema, created as the next revision when
/// none does. Revisions are immutable: an identical schema (by hash of its
/// canonical JSON) reuses its row, any change makes a new one.
pub async fn revision_for(
    db: &DatabaseConnection,
    app_id: Uuid,
    event_name: &str,
    schema: &Value,
) -> Result<app_event_schemas::Model, DbErr> {
    let sha256 = schema_sha256(schema);
    let by_sha = || {
        app_event_schemas::Entity::find()
            .filter(app_event_schemas::Column::AppId.eq(app_id))
            .filter(app_event_schemas::Column::EventName.eq(event_name))
            .filter(app_event_schemas::Column::Sha256.eq(sha256.as_str()))
            .one(db)
    };
    // A concurrent publish of the same event can take the next revision
    // number or the same hash between the read and the insert; the unique
    // indexes refuse the loser, who looks again.
    let mut last = None;
    for _ in 0..4 {
        if let Some(found) = by_sha().await? {
            // A reuse counts as a use: retention leaves a revision recorded
            // in the last hour alone, so it is normally not pruned under this
            // publish. (`created_at` is read as "last recorded"; the order
            // of revisions is `revision`.)
            let stamped = app_event_schemas::Entity::update_many()
                .col_expr(
                    app_event_schemas::Column::CreatedAt,
                    sea_orm::sea_query::Expr::current_timestamp().into(),
                )
                .filter(app_event_schemas::Column::Id.eq(found.id))
                .exec(db)
                .await?;
            if stamped.rows_affected == 1 {
                return Ok(found);
            }
            // Retention deleted it between the read and the stamp: look
            // again, which records the schema afresh.
            continue;
        }
        let latest = app_event_schemas::Entity::find()
            .filter(app_event_schemas::Column::AppId.eq(app_id))
            .filter(app_event_schemas::Column::EventName.eq(event_name))
            .order_by_desc(app_event_schemas::Column::Revision)
            .one(db)
            .await?;
        let inserted = app_event_schemas::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(app_id),
            event_name: Set(event_name.to_string()),
            revision: Set(latest.map_or(1, |l| l.revision + 1)),
            schema: Set(schema.clone()),
            sha256: Set(sha256.clone()),
            created_at: Set(Utc::now().into()),
        }
        .insert(db)
        .await;
        match inserted {
            Ok(row) => return Ok(row),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| DbErr::Custom("event schema revision not recorded".into())))
}

/// The revision the build `environment` serves now declares for the event:
/// the one whose hash matches the schema in that build's manifest. `None`
/// when the environment serves no build, or its build does not declare the
/// event.
pub async fn live_revision(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &str,
    event_name: &str,
) -> Result<Option<app_event_schemas::Model>, DbErr> {
    let Some(row) = app_environments::Entity::find_by_id((app_id, environment.to_string()))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let Some(build_id) = row.build_id else {
        return Ok(None);
    };
    let Some(build) = app_builds::Entity::find_by_id(build_id).one(db).await? else {
        return Ok(None);
    };
    let Ok(catalog) = EventCatalog::from_manifest(build.manifest_json.as_ref()) else {
        return Ok(None);
    };
    let Some(decl) = catalog.events.get(event_name) else {
        return Ok(None);
    };
    recorded_revision(db, app_id, event_name, &decl.schema).await
}

/// The revision recorded for the event holding exactly `schema` (by the hash
/// of its canonical JSON), without recording one: `None` when no publish of a
/// build declaring it has. What a function's publish validates against is
/// the running build's declaration, found here.
pub async fn recorded_revision(
    db: &DatabaseConnection,
    app_id: Uuid,
    event_name: &str,
    schema: &Value,
) -> Result<Option<app_event_schemas::Model>, DbErr> {
    app_event_schemas::Entity::find()
        .filter(app_event_schemas::Column::AppId.eq(app_id))
        .filter(app_event_schemas::Column::EventName.eq(event_name))
        .filter(app_event_schemas::Column::Sha256.eq(schema_sha256(schema)))
        .one(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removing_a_field_is_incompatible() {
        let old = json!({"type":"object","properties":{"period":{"type":"string"}}});
        let new = json!({"type":"object","properties":{}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`period` was removed".to_string()]
        );
    }

    #[test]
    fn adding_an_optional_field_is_compatible() {
        let old = json!({"type":"object","properties":{"period":{"type":"string"}}});
        let new = json!({"type":"object","properties":{
            "period":{"type":"string"},"note":{"type":"string"}}});
        assert!(incompatibilities(&old, &new).is_empty());
    }

    #[test]
    fn changing_a_type_is_incompatible() {
        let old = json!({"type":"object","properties":{"count":{"type":"integer"}}});
        let new = json!({"type":"object","properties":{"count":{"type":"string"}}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`count` changed type from integer to string".to_string()]
        );
    }

    #[test]
    fn making_a_field_required_is_incompatible() {
        let old = json!({"type":"object","properties":{"period":{"type":"string"}}});
        let new = json!({"type":"object","properties":{"period":{"type":"string"}},
            "required":["period"]});
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`period` became required".to_string()]
        );
    }

    #[test]
    fn nested_paths_are_named() {
        let old = json!({"type":"object","properties":{"location":{
            "type":"object","properties":{"id":{"type":"string"}}}}});
        let new = json!({"type":"object","properties":{"location":{
            "type":"object","properties":{}}}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`location.id` was removed".to_string()]
        );
    }

    fn lines(amount: Value) -> Value {
        json!({"type":"object","properties":{"lines":{"type":"array","items":{
            "type":"object","properties":amount}}}})
    }

    #[test]
    fn removing_a_field_inside_an_array_element_is_incompatible() {
        let old = lines(json!({"amount":{"type":"number"}}));
        let new = lines(json!({}));
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`lines[].amount` was removed".to_string()]
        );
    }

    #[test]
    fn retyping_a_field_inside_an_array_element_is_incompatible() {
        let old = lines(json!({"amount":{"type":"number"}}));
        let new = lines(json!({"amount":{"type":"string"}}));
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`lines[].amount` changed type from number to string".to_string()]
        );
    }

    #[test]
    fn retyping_an_arrays_elements_or_a_tuple_slot_is_incompatible() {
        let old = json!({"type":"object","properties":{
            "tags":{"type":"array","items":{"type":"string"}},
            "pair":{"type":"array","prefixItems":[{"type":"string"},{"type":"number"}]}}});
        let new = json!({"type":"object","properties":{
            "tags":{"type":"array","items":{"type":"integer"}},
            "pair":{"type":"array","prefixItems":[{"type":"string"},{"type":"string"}]}}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec![
                "`tags[]` changed type from string to integer".to_string(),
                "`pair[1]` changed type from number to string".to_string(),
            ]
        );
    }

    fn money(defs: Value) -> Value {
        json!({"type":"object","properties":{"total":{"$ref":"#/$defs/Money"}},"$defs":defs})
    }

    #[test]
    fn a_change_behind_a_local_ref_is_incompatible() {
        let old = money(json!({"Money":{"type":"object","properties":{
            "amount":{"type":"integer"},"currency":{"type":"string"}}}}));
        let removed = money(json!({"Money":{"type":"object","properties":{
            "amount":{"type":"integer"}}}}));
        assert_eq!(
            incompatibilities(&old, &removed),
            vec!["`total.currency` was removed".to_string()]
        );
        let retyped = money(json!({"Money":{"type":"object","properties":{
            "amount":{"type":"string"},"currency":{"type":"string"}}}}));
        assert_eq!(
            incompatibilities(&old, &retyped),
            vec!["`total.amount` changed type from integer to string".to_string()]
        );
        // The ref itself replaced by another type: the type is the
        // definition's.
        let inline = json!({"type":"object","properties":{"total":{"type":"string"}}});
        let found = incompatibilities(&old, &inline);
        assert_eq!(
            found[0], "`total` changed type from object to string",
            "{found:?}"
        );
        // The same shape, written inline or behind a ref, is no change.
        let same_inline = json!({"type":"object","properties":{"total":{"type":"object",
            "properties":{"amount":{"type":"integer"},"currency":{"type":"string"}}}}});
        assert!(incompatibilities(&old, &same_inline).is_empty());
        assert!(check_schema(&old).is_ok(), "a local ref compiles");
    }

    #[test]
    fn all_of_members_are_walked() {
        let old = json!({"type":"object","allOf":[
            {"properties":{"a":{"type":"string"}}},
            {"properties":{"b":{"type":"integer"}}}]});
        let new = json!({"type":"object","allOf":[
            {"properties":{"a":{"type":"string"}},"required":["a"]}]});
        assert_eq!(
            incompatibilities(&old, &new),
            vec![
                "`a` became required".to_string(),
                "`b` was removed".to_string()
            ]
        );
    }

    #[test]
    fn a_recursive_ref_ends() {
        let tree = json!({"type":"object","properties":{
            "name":{"type":"string"},"children":{"type":"array","items":{"$ref":"#"}}}});
        assert!(incompatibilities(&tree, &tree).is_empty());
        let renamed = json!({"type":"object","properties":{
            "label":{"type":"string"},"children":{"type":"array","items":{"$ref":"#"}}}});
        // Reported once, where it is declared, not once per level of nesting.
        assert_eq!(
            incompatibilities(&tree, &renamed),
            vec!["`name` was removed".to_string()]
        );
    }

    /// Runs `f` on a thread of its own and fails past `limit`: an
    /// exponential walk would otherwise hold the test for hours. Generous:
    /// each shape below takes microseconds bounded and hours unbounded.
    fn within<T: Send + 'static>(
        limit: std::time::Duration,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(f());
        });
        finished
            .recv_timeout(limit)
            .unwrap_or_else(|_| panic!("the comparison did not finish within {limit:?}"))
    }

    const ONE_SECOND: std::time::Duration = std::time::Duration::from_secs(1);

    /// R33: an `allOf` whose members both refer back to their own schema
    /// doubled the gathering at every level, down to the depth limit. Each
    /// node is gathered once per view now.
    #[test]
    fn an_all_of_referring_to_itself_twice_is_compared_at_once() {
        let schema = json!({"type":"object","properties":{"x":{"$ref":"#/$defs/A"}},
            "$defs":{"A":{"allOf":[{"$ref":"#/$defs/A"},{"$ref":"#/$defs/A"}]}}});
        assert!(
            check_schema(&schema).is_ok(),
            "the shape passes check_schema"
        );
        let copy = schema.clone();
        assert!(within(ONE_SECOND, move || incompatibilities(&schema, &copy)).is_empty());
    }

    /// `D_i.properties.{a,b}` both refer to `D_{i+1}`, forty levels deep.
    fn shared_chain(leaf: &str) -> Value {
        let mut defs = serde_json::Map::new();
        for i in 0..40 {
            let next = json!({"$ref": format!("#/$defs/D{}", i + 1)});
            defs.insert(
                format!("D{i}"),
                json!({"type":"object","properties":{"a":next.clone(),"b":next}}),
            );
        }
        defs.insert(
            "D40".into(),
            json!({"type":"object","properties":{"leaf":{"type":leaf}}}),
        );
        json!({"type":"object","properties":{"top":{"$ref":"#/$defs/D0"}},"$defs":defs})
    }

    /// R33: a chain shared at every level cost 2^depth walks, as the walk
    /// remembered only the pairs it was inside. Each pair is compared once
    /// now — and a change at the bottom is still found, once.
    #[test]
    fn a_chain_shared_at_every_level_is_compared_at_once() {
        let (old, new) = (shared_chain("string"), shared_chain("string"));
        assert!(check_schema(&old).is_ok(), "the shape passes check_schema");
        assert!(within(ONE_SECOND, move || incompatibilities(&old, &new)).is_empty());

        let (old, new) = (shared_chain("string"), shared_chain("integer"));
        let found = within(ONE_SECOND, move || incompatibilities(&old, &new));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].ends_with(".a.leaf` changed type from string to integer"),
            "{found:?}"
        );
    }

    /// R33: past its budget the walk stops and fails closed — one finding,
    /// which `--break-listeners` overrides like any other — rather than
    /// calling an unread schema compatible. (40,000 properties: well past the
    /// budget, which counts each property several times over; admission
    /// would refuse such a schema for its size, but the walk holds alone.)
    #[test]
    fn past_its_budget_the_comparison_fails_closed() {
        let properties: serde_json::Map<String, Value> = (0..40_000)
            .map(|i| (format!("p{i}"), json!({"type":"string"})))
            .collect();
        let old = json!({"type":"object","properties":properties});
        let new = old.clone();
        assert_eq!(
            within(ONE_SECOND, move || incompatibilities(&old, &new)),
            vec![TOO_COMPLEX.to_string()]
        );
    }

    /// `big` empty properties in `$defs.Big`, and `refs` root properties
    /// each `{"$ref": Big, "type": "object"}`: the sibling makes each its own
    /// schema, so each one's view copies all of Big.
    fn amplified(big: usize, refs: usize) -> Value {
        let big: serde_json::Map<String, Value> =
            (0..big).map(|i| (format!("p{i}"), json!({}))).collect();
        let properties: serde_json::Map<String, Value> = (0..refs)
            .map(|i| {
                (
                    format!("n{i}"),
                    json!({"$ref":"#/$defs/Big","type":"object"}),
                )
            })
            .collect();
        json!({"type":"object","properties":properties,"$defs":{"Big":{
            "type":"object","properties":big}}})
    }

    /// R40: the budget counts work, not nodes: every property a view
    /// gathers, and every property a pair compares, is read from it. A
    /// ~200 KB schema whose refs each copy a 5,000-property definition held
    /// gigabytes and minutes before; it fails closed at once now.
    #[test]
    fn copying_a_wide_definition_into_many_views_fails_closed_at_once() {
        let old = amplified(5_000, 3_300);
        let new = old.clone();
        let found = within(ONE_SECOND, move || incompatibilities(&old, &new));
        assert!(found.contains(&TOO_COMPLEX.to_string()), "{found:?}");
    }

    /// The same shape within the 16 KiB an event schema may be: still over
    /// the budget, still closed.
    #[test]
    fn the_amplifying_shape_at_the_size_cap_fails_closed() {
        let old = amplified(400, 200);
        let size = serde_json::to_vec(&old).unwrap().len();
        assert!(
            size <= crate::custom_apps_function_schema::MAX_SCHEMA_BYTES,
            "{size}"
        );
        let new = old.clone();
        let found = within(ONE_SECOND, move || incompatibilities(&old, &new));
        assert_eq!(found, vec![TOO_COMPLEX.to_string()]);
    }

    /// A schema as wide as the size cap allows, with nothing amplifying it,
    /// is compared in full: the budget is far above an honest schema.
    #[test]
    fn a_wide_schema_at_the_size_cap_is_compared_in_full() {
        let properties: serde_json::Map<String, Value> = (0..600)
            .map(|i| (format!("f{i}"), json!({"type":"string"})))
            .collect();
        let old = json!({"type":"object","properties":properties});
        assert!(
            serde_json::to_vec(&old).unwrap().len()
                <= crate::custom_apps_function_schema::MAX_SCHEMA_BYTES
        );
        let mut new = old.clone();
        new["properties"].as_object_mut().unwrap().remove("f599");
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`f599` was removed".to_string()]
        );
    }

    /// R40: `items` in `new` where `old` had none constrains elements old
    /// publishers were free to send: a narrowing from anything. After a
    /// closed tuple (`items: false`) they sent none, so nothing narrows.
    #[test]
    fn a_new_items_schema_narrows_from_anything() {
        let old = json!({"type":"object","properties":{"tags":{"type":"array"}}});
        let new = json!({"type":"object","properties":{"tags":{"type":"array",
            "items":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}}}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec![
                "`tags[]` changed type from any to object".to_string(),
                "`tags[].id` became required".to_string(),
            ]
        );
        let closed = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"}],"items":false}}});
        let opened = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"}],"items":{"type":"string"}}}});
        assert!(incompatibilities(&closed, &opened).is_empty());
    }

    /// R40: a tuple slot `new` removes falls under new's `items`, so it is
    /// held against them; after a closed tuple the slot is refused outright
    /// (removed); with no `items` it is unconstrained, a widening.
    #[test]
    fn a_tuple_slot_new_removes_is_held_against_new_items() {
        // Elements past the tuple are strings in both, so only the slot moves.
        let old = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"},{"type":"number"}],"items":{"type":"string"}}}});
        let held = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"}],"items":{"type":"string"}}}});
        assert_eq!(
            incompatibilities(&old, &held),
            vec!["`pair[1]` changed type from number to string".to_string()]
        );
        let closed = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"}],"items":false}}});
        // Closing it also forbids the strings old allowed past the tuple.
        assert_eq!(
            incompatibilities(&old, &closed),
            vec![
                "`pair[]` was removed".to_string(),
                "`pair[1]` was removed".to_string(),
            ]
        );
        let open = json!({"type":"object","properties":{"pair":{"type":"array",
            "prefixItems":[{"type":"string"}]}}});
        assert!(incompatibilities(&old, &open).is_empty());
    }

    /// Closing a tuple (`items: false`) forbids the elements old publishers
    /// were free to send past it — under old's `items`, or anything when old
    /// had none: a narrowing, reported as their removal. A tuple closed in
    /// both narrows nothing.
    #[test]
    fn closing_an_open_tuple_is_incompatible() {
        let tuple = |items: Option<Value>| {
            let mut pair = json!({"type":"array","prefixItems":[{"type":"string"}]});
            if let Some(items) = items {
                pair["items"] = items;
            }
            json!({"type":"object","properties":{"pair":pair}})
        };
        let closed = tuple(Some(json!(false)));
        for open in [tuple(Some(json!({"type":"number"}))), tuple(None)] {
            assert_eq!(
                incompatibilities(&open, &closed),
                vec!["`pair[]` was removed".to_string()],
                "{open}"
            );
        }
        assert!(incompatibilities(&closed, &closed.clone()).is_empty());
    }

    /// R42 (N3): an honest schema as large as the 16 KiB cap allows, built
    /// by inheritance — a 15-property base with 5 required names, and 100
    /// subtypes each `allOf: [base, 4 more properties]` — is compared in
    /// full: the budget is far above honest work, and only amplification
    /// reaches it.
    #[test]
    fn an_honest_inheritance_schema_at_the_size_cap_is_compared_in_full() {
        let inheriting = |dropped: Option<usize>| {
            let base: serde_json::Map<String, Value> = (0..15)
                .map(|i| (format!("b{i}"), json!({"type":"string"})))
                .collect();
            let subtypes: serde_json::Map<String, Value> = (0..100)
                .map(|i| {
                    let own: serde_json::Map<String, Value> = (0..4)
                        .filter(|&j| dropped != Some(i * 4 + j))
                        .map(|j| (format!("c{i}{j}"), json!({"type":"string"})))
                        .collect();
                    (
                        format!("t{i}"),
                        json!({"allOf":[{"$ref":"#/$defs/B"},{"properties":own}]}),
                    )
                })
                .collect();
            json!({"type":"object","properties":subtypes,"$defs":{"B":{
                "type":"object","properties":base,"required":["b0","b1","b2","b3","b4"]}}})
        };
        let old = inheriting(None);
        let size = serde_json::to_vec(&old).unwrap().len();
        assert!(
            size > 12 * 1024 && size <= crate::custom_apps_function_schema::MAX_SCHEMA_BYTES,
            "{size}"
        );
        let same = old.clone();
        assert!(within(ONE_SECOND, move || incompatibilities(&old, &same)).is_empty());
        // A change in the last subtype is still found.
        let (old, new) = (inheriting(None), inheriting(Some(399)));
        assert_eq!(
            within(ONE_SECOND, move || incompatibilities(&old, &new)),
            vec!["`t99.c993` was removed".to_string()]
        );
    }

    /// Past the deepest level the walk goes, likewise: a change below it
    /// is not called compatible unread.
    #[test]
    fn past_its_depth_the_comparison_fails_closed() {
        let chain = |leaf: &str| {
            let mut defs = serde_json::Map::new();
            for i in 0..70 {
                defs.insert(
                    format!("L{i}"),
                    json!({"type":"object","properties":{
                        "next":{"$ref": format!("#/$defs/L{}", i + 1)}}}),
                );
            }
            defs.insert(
                "L70".into(),
                json!({"type":"object","properties":{"v":{"type":leaf}}}),
            );
            json!({"type":"object","properties":{"head":{"$ref":"#/$defs/L0"}},"$defs":defs})
        };
        assert_eq!(
            incompatibilities(&chain("string"), &chain("integer")),
            vec![TOO_COMPLEX.to_string()]
        );
    }

    /// R33: keywords beside a `$ref` (2020-12 allows them) are part of the
    /// schema, as `allOf: [target, siblings]`: a property removed from
    /// beside a ref is a removal.
    #[test]
    fn a_property_removed_beside_a_ref_is_incompatible() {
        let defs = json!({"Money":{"type":"object","properties":{"amount":{"type":"integer"}}}});
        let old = json!({"type":"object","properties":{"total":{"$ref":"#/$defs/Money",
            "properties":{"note":{"type":"string"}}}},"$defs":defs});
        let new = json!({"type":"object","properties":{"total":{"$ref":"#/$defs/Money"}},
            "$defs":defs});
        assert!(check_schema(&old).is_ok());
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`total.note` was removed".to_string()]
        );
        // The target's own properties still count.
        let retyped = json!({"type":"object","properties":{"total":{"$ref":"#/$defs/Money",
            "properties":{"note":{"type":"string"}}}},
            "$defs":{"Money":{"type":"object","properties":{"amount":{"type":"string"}}}}});
        assert_eq!(
            incompatibilities(&old, &retyped),
            vec!["`total.amount` changed type from integer to string".to_string()]
        );
    }

    /// R33: a tuple slot `new` adds is held against what `old` allowed
    /// there: old's `items`, or anything when old has none. A closed tuple
    /// (`items: false`) sent nothing there, so a slot added to it narrows
    /// nothing.
    #[test]
    fn an_added_tuple_slot_is_held_against_old_items() {
        let old = json!({"type":"object","properties":{
            "pair":{"type":"array","items":{"type":"number"}}}});
        let new = json!({"type":"object","properties":{
            "pair":{"type":"array","prefixItems":[{"type":"integer"}],"items":{"type":"number"}}}});
        assert_eq!(
            incompatibilities(&old, &new),
            vec!["`pair[0]` changed type from number to integer".to_string()]
        );

        let open = json!({"type":"object","properties":{
            "pair":{"type":"array","prefixItems":[{"type":"string"}]}}});
        let added = json!({"type":"object","properties":{
            "pair":{"type":"array","prefixItems":[{"type":"string"},
                {"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}]}}});
        assert_eq!(
            incompatibilities(&open, &added),
            vec![
                "`pair[1]` changed type from any to object".to_string(),
                "`pair[1].a` became required".to_string(),
            ]
        );

        let closed = json!({"type":"object","properties":{
            "pair":{"type":"array","prefixItems":[{"type":"string"}],"items":false}}});
        let widened = json!({"type":"object","properties":{
            "pair":{"type":"array","prefixItems":[{"type":"string"},{"type":"boolean"}],
                "items":false}}});
        assert!(incompatibilities(&closed, &widened).is_empty());
    }

    #[test]
    fn validate_refuses_a_missing_required_field() {
        let schema = json!({"type":"object","properties":{"period":{"type":"string"}},
            "required":["period"]});
        let errors = validate(&schema, &json!({})).expect_err("period is missing");
        assert!(errors.iter().any(|e| e.contains("period")), "{errors:?}");
        assert!(validate(&schema, &json!({"period":"2026-Q3"})).is_ok());
    }

    /// R36: a violation names the instance path and the keyword it failed,
    /// never the value: a payload is the app's data, and a refusal's text
    /// reaches logs, run records and replies. A missing property is named by
    /// its path, from the schema.
    #[test]
    fn a_violation_names_its_path_and_keyword_never_the_value() {
        let schema = json!({"type":"object","properties":{
            "customer":{"type":"string"},
            "tags":{"type":"array","items":{"type":"integer"}},
            "period":{"type":"string"}},
            "required":["period"],"additionalProperties":false});
        let data = json!({
            "customer":{"name":"Ada Lovelace","card":"4242 4242 4242 4242"},
            "tags":[1, "Babbage"],
            "secretKey":"hunter2"});
        let errors = validate(&schema, &data).expect_err("three violations and more");
        let joined = errors.join("; ");
        for part in [
            "Ada",
            "Lovelace",
            "4242",
            "name",
            "card",
            "Babbage",
            "hunter2",
            "secretKey",
        ] {
            assert!(!joined.contains(part), "{part} in {joined}");
        }
        for expected in [
            "/customer fails `type`",
            "/tags/1 fails `type`",
            "/period fails `required`",
            "/ fails `additionalProperties`",
        ] {
            assert!(
                errors.iter().any(|e| e == expected),
                "{expected} in {errors:?}"
            );
        }
    }

    /// R40: a refusal's path names only what the schema declares. A key the
    /// payload chose — under `additionalProperties` or `patternProperties`,
    /// an email, a card number, a numeric id — is written `*`; an array
    /// index and a declared property name stay.
    #[test]
    fn a_violation_path_hides_keys_the_payload_chose() {
        let schema = json!({"type":"object","properties":{
            "balances":{"type":"object","additionalProperties":{"type":"integer"}},
            "tags":{"type":"array","items":{"type":"object",
                "patternProperties":{"^x-":{"type":"string"}}}}}});
        let data = json!({
            "balances":{"ada@example.com":"x","4242424242424242":"y","balances":"z"},
            "tags":[{"x-ada@example.com":1}]});
        let errors = validate(&schema, &data).expect_err("four violations");
        let joined = errors.join("; ");
        for part in ["ada", "example", "4242"] {
            assert!(!joined.contains(part), "{part} in {joined}");
        }
        let mut sorted = errors.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec![
                "/balances/* fails `type`".to_string(),
                "/balances/* fails `type`".to_string(),
                // Declared elsewhere in the schema, so a schema word, kept.
                "/balances/balances fails `type`".to_string(),
                "/tags/0/* fails `type`".to_string(),
            ]
        );
    }

    #[test]
    fn check_schema_refuses_a_non_object_root() {
        assert!(check_schema(&json!({"type":"string"})).is_err());
        assert!(check_schema(&json!({"properties":{}})).is_err());
        assert!(check_schema(&json!({"type":"object","properties":{}})).is_ok());
        // Not a schema at all: `properties` must be an object.
        assert!(check_schema(&json!({"type":"object","properties":3})).is_err());
    }

    #[test]
    fn check_schema_refuses_a_remote_ref_without_fetching_it() {
        let schema = json!({"type":"object","properties":{
            "x":{"$ref":"https://example.invalid/schema.json"}}});
        assert!(check_schema(&schema).is_err());
    }

    #[test]
    fn the_hash_ignores_key_order() {
        let a = json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}}});
        let b = json!({"properties":{"b":{"type":"string"},"a":{"type":"string"}},"type":"object"});
        assert_eq!(schema_sha256(&a), schema_sha256(&b));
    }
}
