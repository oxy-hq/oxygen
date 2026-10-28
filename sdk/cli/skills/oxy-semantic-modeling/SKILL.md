---
name: oxy-semantic-modeling
description: >-
  Use when writing, reviewing, or debugging an Oxy semantic model — `.view.yml` views,
  `.topic.yml` topics, measures, dimensions, entities, pre-aggregations (rollups), and the
  `meta:` freshness block — in a customer workspace, and when one misbehaves: a measure that
  vanishes after switching topics, "measure not found", a topic returning no rows or missing
  joins, a date filter failing with `TYPE_MISMATCH`, a rollup that never serves, or an agent
  reporting a partial day as a complete number. Carries the craft and the traps the reference
  docs do not.
---

# Oxy semantic modeling

**The reference is upstream and maintained — read it rather than guessing:**
<https://www.oxygen-hq.com/docs/guide/build/semantic-model/simple-model>
(the section entry point; `views`, `entities`, `dimensions`, `measures`,
`topics`, `modeling`, and `use-in-agents` sit beside it).

That page owns the field lists. This skill deliberately does **not** restate
them — a copied schema stays confidently wrong long after the real one moves.
What follows is the craft and the traps it does not carry, with examples taken
from one production workspace's 39 views and 23 topics.

**Read next:**

- [templates/view-template.yml](templates/view-template.yml) and
  [templates/topic-template.yml](templates/topic-template.yml) — annotated
  starting points for a new view or topic.
- [references/warehouse-casts.md](references/warehouse-casts.md) — the
  per-warehouse cast for a date column that is not stored as a date.

## View anatomy

A view is one physical table plus the vocabulary an agent is allowed to use
against it.

```yaml
name: sales_daily
datasource: "clickhouse"
table: "restaurant_analytics.sales_daily_metrics"
```

**`entities` are the join graph, not decoration.** The `primary` entity names
the grain of the row; `foreign` entities are the edges to other views; `parent:`
states hierarchy (a sales day belongs to a restaurant).

```yaml
entities:
  - name: sales_day
    type: primary
    key: sales_day_key
    parent: restaurant
  - name: restaurant
    type: foreign
    key: restaurant_id
```

**`key` names a dimension, not a column.** In a `restaurants` view the entity
`restaurant` has `key: restaurant_id`, and the `restaurant_id` *dimension* is
`expr: guid` — the physical column is `guid`. So resolve a `key` against the
view's `dimensions:` list, never against the table's columns; the reference's
`entities` page carries the same warning.

**Declare a foreign entity for every foreign-key column.** When a fact view
holds an ID pointing at another view's primary key (`orders.restaurant_id` →
`restaurants.guid`), give the fact view a `type: foreign` entity with the
**same `name:`** as the lookup view's primary entity and `key:` set to the FK
dimension. That edge is what lets a query group orders by
`restaurants.location_name` instead of by the raw ID; without it, every chart
and table built on the view renders opaque UUIDs. A foreign entity with no
matching primary anywhere is only a *warning* — the join silently does not
exist.

**`dimensions`** are what you slice by, **`measures`** what you aggregate. Both
take `synonyms`; measures also take `filters` and `drivers`. House style keeps
entity keys single — even a compound natural key is one pre-concatenated
dimension (`sales_day_key`); no view in the corpus uses the documented
composite `keys:`.

## The description is a contract with the agent, not a label

A real `sales_daily` description, verbatim:

> Daily sales metrics aggregated by restaurant and business date. Loaded several
> times daily; the current business day is always intraday-partial - the prior
> full day is the latest complete day.

The second sentence is the whole point. Without it an agent asked "how are sales
today?" reports a half-loaded partial day as a real number, confidently and with
no warning — wrong in a way nobody can see downstream. Write descriptions that
say when the data is complete and what it excludes, not what the table holds.

## The `meta:` freshness block

Four keys that state a view's own lag characteristics, so the analytics agent
can qualify an answer instead of guessing:

```yaml
meta:
  freshness_watermark_column: ["business_date"]
  freshness_expected_cadence: ["several times daily"]
  freshness_complete_through: ["prior full day (America/Los_Angeles)"]
  freshness_caveat: ["Current business day is intraday-partial."]
```

**Only 11 of 39 views carry one.** That low number is the point: it is the most
under-used high-value feature in a real model, and adding one is the cheapest
quality win available on any view fed by a batch pipeline.

- **`meta:` is a schema field; the four key names are a convention.** `meta`
  exists on views (and dimensions, measures, entities, segments and topics),
  typed as a map from a key to a **list of strings**. That is why every value
  is a one-element list: a bare string (`freshness_caveat: "…"`) fails to
  parse. The four `freshness_*` names appear in no docs page; the analytics
  agent reads them by name and uses the first element of each.
- **The watermark resolves against dimensions first.**
  `freshness_watermark_column` may name a dimension (or match its `expr`); if
  it matches neither, it is used as a raw column. **Without the key, the agent
  falls back to the view's first `date`/`datetime` dimension** — which may be
  an order date rather than the load watermark, so declare it.
- A view-level `refresh_key: { sql: … }` (see Pre-aggregations) doubles as the
  agent's recency probe for the view.

## Craft

**`synonyms` are the natural-language surface.** Map both domain jargon and plain
English onto one name; colliding common words across views is normal and wanted.

```yaml
  - name: total_net_sales
    synonyms: ["net sales", "net revenue", "revenue", "sales"]
  - name: sales_per_guest
    synonyms: ["per guest average", "guest average", "PPA"]
```

**`samples` on enum-ish dimensions** tell the agent the valid values, so it
filters on `"PAID"` instead of inventing `"paid"`. Give the *full* set, not a
sample — `["OPEN", "PAID", "CLOSED"]`, `["negative", "neutral", "positive"]`.

**`samples` is always a list of strings — even on a `boolean` or `number`
dimension.** Write `samples: ["true", "false"]` and `samples: ["5", "10"]`,
never `[true, false]` or `[5, 10]`. The field only accepts strings, and one
bare literal makes the view fail to load — and with it the whole semantic
model, not just that view. The reference's `dimensions` page shows a numeric
example with bare numbers; do not copy it.

**"Aggregate X where Y" is a filtered measure, not a `custom` `CASE WHEN`.**
Write a typed measure (`sum`, `count`, `average`, …) with a `filters:` list:

```yaml
  - name: delivery_revenue
    type: sum
    expr: order_amount
    filters:
      - expr: "order_type = 'delivery'"   # raw column SQL, or {{dimension}} — see Traps
```

It reads better, and it re-aggregates: a filtered `sum` can be answered from a
pre-aggregation, while a `custom` measure never can (nor can `median` or
`number`). Reserve `custom` for what a filtered measure cannot express —
ratios, correlations, cross-period math.

**Derived time dimensions so the agent never writes date arithmetic.** Ship
`business_year`, `business_month`, `business_quarter`, `day_of_week`,
`is_weekend` as dimensions of their own, each re-derived from the raw column
rather than referencing the base date dimension:

```yaml
  - name: business_quarter
    expr: "toQuarter(toDate(parseDateTimeBestEffort(toString(business_date))))"
```

**`drivers` must state a mechanism, not a vibe.** The quality bar:

```yaml
  - name: total_net_sales
    drivers:
      - measure: sales_daily.discount_rate
        direction: negative
        strength: strong
        confidence: high
        description: "A larger share of gross sales given away as discounts lowers net sales. Unlike discount dollars, the rate does not move with volume."
```

Note what the `description` does: it names the causal path *and* says why the
obvious alternative (discount dollars) was rejected. The weak examples in the
corpus carry `direction`, `strength`, `confidence`, and a `lag`, but no
`description` at all, so the mechanism is never stated and a reader cannot tell
whether the edge was reasoned or guessed. A driver without a `description` is
not finished. Only 8 of 39 views use `drivers`, and the field is absent from
the semantic-model docs — see the traps.

## Dates: declare them as dates

A dimension whose meaning is a date or timestamp (`*_date`, `*_at`,
`business_date`, `created`, `event_time`, …) must be `type: date` (or
`type: datetime`), and its `expr` must produce a real date — not the raw
column, if the column is stored as a string, an integer like `20240131`, or
epoch seconds. Declare it `type: number` or `type: string` instead and nothing
complains when the view loads; the first query that filters on it compiles a
date literal the column rejects and fails with **`TYPE_MISMATCH`**.

Sample before you cast — stored formats differ between warehouses and between
tables in one warehouse, and one `SELECT <col> FROM <table> LIMIT 1` settles it.
Then wrap the `expr` with the cast for your warehouse:
[references/warehouse-casts.md](references/warehouse-casts.md). A column
already typed `DATE` / `TIMESTAMP` needs no cast.

Casting in the view is the fallback, not the goal: when you own the pipeline,
fix the column type there once (see *Repeated defensive casting* below).

## Topics are question areas, not tables

A topic is a `base_view` plus the views it may join to, and the filters that
should always apply. That is the whole shape — all 23 real topics use only
`name`, `description`, `views`, `base_view`, `default_filters`.

```yaml
name: orders
base_view: orders
views:
  - orders
  - order_checks
  - order_selections
  - restaurants
  - dining_options
default_filters:
  - field: "orders.is_voided"
    eq:
      value: false
```

Name topics after the questions people ask, and let `default_filters` carry the
"obviously we don't count voided orders" rules so no one has to remember them.
A topic usually spans several views; "one topic per view" is not the pattern.

**Qualify `default_filters` fields as `view.dimension`.** A bare `field:` is
resolved against a view *named like the topic*, not against `base_view`. It
works when the topic shares its base view's name (as above, where
`is_voided` would also work) and points at nothing when it does not — a topic
`sales` over `base_view: sales_daily` needs `sales_daily.is_voided`.

## Pre-aggregations (rollups)

A rollup is a materialized partial aggregate of one view. When a query is fully
covered by one, the engine answers it from the materialized copy instead of
scanning the warehouse — a few hundred rows instead of millions. They are
worth adding to views behind dashboards and frequently asked KPIs. The
reference documents the `config.yml` block in one line and the view-level
block not at all, so the shape is here.

**Opt-in, per view.** A view with no `pre_aggregations:` block gets **no
rollups at all** — there is no implicit default (an all-dimensions rollup of a
wide view is about as large as the table itself).

```yaml
# in a .view.yml
refresh_key:                     # view-level default for every rollup below
  every: "1h"

pre_aggregations:
  - name: orders_by_month
    dimensions: [order_status]   # names declared in THIS view, never raw columns
    measures: [total_orders, total_order_value]
    time_dimension: order_date   # a date/datetime dimension of this view
    granularity: month           # second|minute|hour|day|week|month|quarter|year
  - name: orders_summary         # no dimensions, no time: one summary row
    measures: [total_orders]
    refresh_key:                 # per-rollup override
      sql: "SELECT MAX(updated_at) FROM orders"
```

- **`refresh_key` takes exactly one of `every` or `sql`** — both, neither, or
  any other key is a parse error. `sql` re-runs against the warehouse every
  cycle and rebuilds when its result changes, so keep it a cheap `MAX`.
- **A rollup that names a member the view does not declare is refused**, not
  quietly trimmed down to the members that resolve.

**When a rollup serves a query.** All of these must hold, or the query falls
back to the warehouse — correct, just not accelerated:

- every requested dimension, and every filtered dimension, is in the rollup's
  `dimensions` (a filter on the `time_dimension` is also fine);
- every requested measure is in its `measures` **and is re-aggregatable** —
  `count`, `count_distinct`, `sum`, `average`, `min`, `max`. A `custom`,
  `number` or `median` measure never serves from a rollup, listed or not;
- the time dimension matches, and the requested granularity is the stored one
  or coarser (a `day` rollup serves `month`; a `month` rollup cannot serve
  `day`);
- **except `week`**: a `week` rollup does not serve `month`, `quarter` or
  `year`, because a week straddling a month boundary would land in the wrong
  month. Store `day` if you need both weekly and monthly answers.

**Limits.** A rollup belongs to one view and holds only that view's members,
and granularities are the fixed list above — no 15-minute buckets. For a
multi-view aggregate or a custom bucket, build a rollup *table* with an
automation instead (`oxy-data-architecture` owns that pattern).

**`config.yml` block** — every key optional:

```yaml
pre_aggregations:
  schema: AIRLAYER            # where rollup tables are built (default AIRLAYER)
  database: warehouse         # connector for builds (default: each view's own datasource)
  refresh_worker:
    enabled: true             # default true
    heartbeat: "10m"          # how often staleness is checked (default 10m, clamped 30s–24h)
    renewal_threshold: "120s" # how long a refresh_key result is trusted (default 120s, clamped 10s–1h)
```

## Validating a change

- **`oxy validate --file semantics/views/orders.view.yml`** parses one view or
  topic file and reports where it fails — a bare boolean in `samples`, a
  missing `entities:` or `dimensions:` list, a removed feature. Run it on every
  file you touch. It does **not** look inside `pre_aggregations:` or
  `refresh_key:`; mistakes there surface only when the model is loaded for a
  query.
- Bare **`oxy validate`** (every file) only looks in `semantics/views/` and
  `semantics/topics/`, while views and topics anywhere in the workspace
  compile. A view kept elsewhere is only checked with `--file`.
- Neither checks **cross-file** wiring — an entity join that matches nothing,
  a topic naming a view that does not exist, a measure that a twin lacks. Run
  a real query through the topic before calling it done.
- **`oxy build` does not validate the semantic model** — it does nothing any
  more. `oxy sync` writes table and column
  metadata to `.databases/`, which is the place to read real column names
  from.
- `inherits_from:` and `{{ globals.* }}` were removed from views; a file using
  either is refused at load. Inline the values.

## Traps

**Twin views from a backend migration are not mirrors.** In the reference
workspace a ClickHouse → Airhouse migration is in flight, so **11 of the 39
views exist twice** (22 views, 11 twin pairs) — plus 3 further `airhouse_*`
views with no ClickHouse counterpart at all, where discount- and payment-level
detail lives only on the new side. Three consequences, in order of how much
time they cost:

1. **The twins diverge.** `sales_daily` defines a `discount_rate` measure;
   `airhouse_sales_daily` does not. Switch topics mid-conversation from one
   family to the other and a query that worked seconds ago fails with "measure
   not found", with nothing anywhere saying the two views were ever supposed to
   match. Before assuming a measure exists on a twin, read the twin.
2. **The two families cannot join to each other, and fail silently when you
   try.** Every `airhouse_*` view prefixes its **foreign** entity names —
   the edges — as `airhouse_restaurant`, `airhouse_order`, `airhouse_check`,
   while its twin uses bare `restaurant`, `order`, `check`. Joins resolve by
   matching entity *names*, so a topic mixing families does not error — it just
   fails to auto-join. You get missing rows, not a config failure. Keep each
   topic within one family.

   **The prefix is not universal, and the exception runs the other way.** The
   three twin-less views leave their **primary** entity bare
   (`discount_application`, `payment`, `selection_discount_application`).
   Their foreign entities are still prefixed, so rule 2 holds — but the latent
   failure inverts. A bare primary is exactly what an old-family view would
   declare, so if anyone adds a `payment` entity on the old side, the two
   families start auto-joining *across* the divide: wrong rows silently,
   instead of missing rows silently. Prefix the primary too on any new view in
   a migrated family.
3. **Do not copy the pattern.** Duplicating a view to change backends is a
   migration tactic with an end date, not a modelling technique.

**Repeated defensive casting is a smell.** In the reference `sales_daily`,
`toDate(parseDateTimeBestEffort(toString(business_date)))` appears in **6
derived dimensions** — the same three-deep cast chain re-typed six times
because the underlying column is not a date. Fix the column type once,
upstream in the pipeline, rather than in every `expr`. (Counting by grep
over-reports: a seventh occurrence is inside `measures:`, not `dimensions:`.)

**`drivers` and `parent:` are documented, but not where you would look.** Neither
appears in the semantic-model pages — `drivers` is explained under
<https://www.oxygen-hq.com/docs/guide/build/world-model/metric-tree> and
`parent:` under <https://www.oxygen-hq.com/docs/guide/build/world-model>. Worse,
the metric-tree page links back to `measures` claiming it defines the measures
"and their drivers", which it does not. Read only the semantic-model section and
you will conclude both fields do not exist and model without them.

**`{{ }}` references: bare for the same view, qualified for another.** Inside a
measure's `expr` or `filters:`, `{{is_voided}}` names a dimension or measure
of the *same* view and is expanded for you — that is the norm in the corpus.
`{{view.member}}` reaches another view's member and is the rare case: across
39 views it appears in exactly one expression (`discount_rate`, which uses it
twice on one line, so a grep shows 2 hits from 1 site). A `{{ }}` that names
nothing resolvable fails the query with an "unresolved reference" error rather
than reaching the warehouse. Older guidance that says never to put `{{ }}` in a
filter predates this and is wrong; raw column SQL (`status = 'completed'`)
remains fine too.

**Do not add a `# yaml-language-server: $schema=…` header.** No JSON schema is
published for `.view.yml` or `.topic.yml`; a header pointing at one makes an
editor validate against nothing — or against the wrong file — while looking as
if it works.
