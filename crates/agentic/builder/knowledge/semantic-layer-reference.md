---
source:
  - sdk/cli/skills/oxy-semantic-modeling/SKILL.md
reconciled-hash: eda741a8ea6cd05a46bedd9ca547754870ff88a5
note: |
  Authored condensation of the platform skills listed above. Not auto-synced —
  scripts/sync-skills.sh only copies the verbatim YAML templates. Re-condense
  when a source changes; keep under ~200 lines so the LLM context stays focused.
  scripts/check-skills-drift.sh flags a changed source and prints the hash to record.
---

# Oxy Semantic Model Reference

The semantic model is two YAML file types:

- `*.view.yml` — one physical table plus the vocabulary (entities, typed
  dimensions, measures) an agent may use against it.
- `*.topic.yml` — a question area: a `base_view`, the views it may join to,
  and filters that always apply.

Views and topics **anywhere in the workspace** compile; `semantics/views/` and
`semantics/topics/` is the conventional layout (and the only place a bare
`oxy validate` looks). Upstream field reference:
<https://www.oxygen-hq.com/docs/guide/build/semantic-model/simple-model>.

## View anatomy

```yaml
name: sales_daily
description: >-
  Daily sales by restaurant and business date. Loaded several times daily;
  the current business day is always intraday-partial - the prior full day
  is the latest complete day.
datasource: "clickhouse"            # a database name from config.yml
table: "restaurant_analytics.sales_daily_metrics"

entities:
  - name: sales_day
    type: primary                   # the grain of the row — exactly one
    key: sales_day_key              # a DIMENSION name, never a raw column
    parent: restaurant              # hierarchy: a sales day belongs to a restaurant
  - name: restaurant
    type: foreign                   # an edge to restaurants.view.yml
    key: restaurant_id

dimensions:
  - name: sales_day_key
    type: string                    # string|number|date|datetime|boolean
    expr: sales_day_key
  - name: restaurant_id
    type: string
    expr: restaurant_id
  - name: order_status
    type: string
    expr: status
    samples: ["OPEN", "PAID", "CLOSED"]   # full value set, always strings
    synonyms: ["status"]

measures:
  - name: order_count
    type: count                     # the only type that omits `expr`
  - name: total_net_sales
    type: sum
    expr: net_sales
    synonyms: ["net sales", "net revenue", "revenue", "sales"]
```

**Rules:**

1. **Entities are the join graph.** Every view declares exactly one
   `type: primary` entity. Joins resolve by matching entity **names** — two
   views join when one's foreign entity has the same `name:` as the other's
   primary. A foreign entity with no matching primary is only a *warning*;
   the join silently does not exist.
2. **`key:` names a dimension, not a column.** In a `restaurants` view the
   entity `restaurant` has `key: restaurant_id`, and the `restaurant_id`
   dimension is `expr: guid`. Resolve keys against `dimensions:`. Keep keys
   single — a compound natural key is one pre-concatenated dimension.
3. **Declare a foreign entity for every FK column.** When a fact view holds
   an ID pointing at another view's primary key (`orders.restaurant_id` →
   `restaurants.guid`), add a `type: foreign` entity with the lookup view's
   primary entity name and `key:` set to the FK dimension. That edge lets a
   query group by `restaurants.location_name`; without it every chart built
   on the view renders opaque UUIDs.
4. **`expr` is required** on every dimension and every measure except
   `count`. Dimension types are lowercase.
5. **`samples` is always a list of strings** — `["true", "false"]`,
   `["5", "10"]`, never bare literals. One bare literal makes the view fail
   to load, and with it the whole semantic model. (The upstream docs show a
   numeric example with bare numbers — don't copy it.)
6. **`inherits_from:` and `{{ globals.* }}` were removed** from views; a file
   using either is refused at load. Inline the values.
7. **Never add a `# yaml-language-server:` header.** No schema is published
   for views or topics, so the header validates against nothing.

Measure types: `count`, `count_distinct`, `sum`, `average`, `min`, `max`
(all re-aggregatable), plus `median`, `number` and `custom` (free-form SQL,
e.g. `CORR(a, b)`), which never serve from a rollup.

## The description is a contract with the agent

Say **when the data is complete and what it excludes**, not what the table
holds. Without "the current business day is intraday-partial", an agent asked
"how are sales today?" confidently reports a half-loaded day as a real number.

## The `meta:` freshness block

```yaml
meta:
  freshness_watermark_column: ["business_date"]
  freshness_expected_cadence: ["several times daily"]
  freshness_complete_through: ["prior full day (America/Los_Angeles)"]
  freshness_caveat: ["Current business day is intraday-partial."]
```

- `meta` (on views, dimensions, measures, entities, segments, topics) maps a
  key to a **list of strings** — a bare string fails to parse. The four
  `freshness_*` names are a convention the analytics agent reads by name
  (first element of each). It's the cheapest quality win on any
  batch-loaded view — add it.
- The watermark resolves against a dimension name or `expr` first, else a raw
  column. **Without it the agent falls back to the view's first
  `date`/`datetime` dimension**, which may be an order date, not the load
  watermark.
- A view-level `refresh_key: { sql: … }` doubles as the agent's recency probe.

## Craft

- **`synonyms`** are the natural-language surface — map jargon and plain
  English onto one name; colliding common words across views is fine.
- **"Aggregate X where Y" is a filtered measure, not a `custom` `CASE WHEN`:**

  ```yaml
  - name: delivery_revenue
    type: sum
    expr: order_amount
    filters:
      - expr: "order_type = 'delivery'"   # raw column SQL, or {{dimension}}
  ```

  It reads better and re-aggregates from a rollup. Reserve `custom` for
  ratios, correlations and cross-period math.
- **`{{ }}` references inside a measure `expr` or `filters:`:** `{{is_voided}}`
  names a dimension/measure of the **same** view and is expanded (the norm);
  `{{view.member}}` reaches another view (rare). A `{{ }}` that names nothing
  resolvable fails with an "unresolved reference" error. Raw column SQL
  (`status = 'completed'`) is equally fine.
- **Derived time dimensions** (`business_year`, `business_month`,
  `business_quarter`, `day_of_week`, `is_weekend`) so the agent never writes
  date arithmetic — each re-derives from the raw column rather than
  referencing the base date dimension.
- **`drivers`** (on measures; documented under the metric-tree docs, as
  `parent:` is under world-model) must state a mechanism:

  ```yaml
  drivers:
    - measure: sales_daily.discount_rate
      direction: negative
      strength: strong
      confidence: high
      description: "A larger share of gross sales given away as discounts lowers net sales. Unlike discount dollars, the rate does not move with volume."
  ```

  A driver without a `description` is not finished.

## Dates: declare them as dates

A dimension meaning a date or timestamp (`*_date`, `*_at`, `business_date`,
`created`, `event_time`, …) must be `type: date` / `datetime`, and its `expr`
must produce a real date. Declared `number`/`string` over a string, integer
(`20240131`) or epoch column, nothing complains at load; the first query that
filters on it fails with **`TYPE_MISMATCH`**. **Sample first**
(`SELECT <col> FROM <table> LIMIT 1`), then cast:

| Warehouse  | Cast functions |
| ---------- | -------------- |
| ClickHouse | `toDate(<col>)`, `toDateTime(<col>)`, `parseDateTimeBestEffort(<col>)`, `toDate(toString(<col>))` for integer dates |
| BigQuery   | `CAST(<col> AS DATE)`, `PARSE_DATE('<fmt>', <col>)`, `PARSE_TIMESTAMP('<fmt>', <col>)`, `TIMESTAMP_SECONDS(<col>)` |
| Snowflake  | `TO_DATE(<col>[, '<fmt>'])`, `TO_TIMESTAMP(<col>)`, `TRY_TO_DATE(<col>)` |
| Postgres   | `(<col>)::date`, `(<col>)::timestamp`, `to_date(<col>, '<fmt>')`, `to_timestamp(<col>)` |
| DuckDB / Airhouse | `CAST(<col> AS DATE)`, `strptime(<col>, '<fmt>')`, `to_timestamp(<col>)`, `epoch_ms(<col>)` |

A column already typed `DATE`/`TIMESTAMP` needs no cast. The same cast chain
repeated in many dimensions is a smell — the column type should be fixed
upstream in the pipeline.

## Topics are question areas, not tables

```yaml
name: orders
description: "Order volume, value and mix by restaurant and dining option"
base_view: orders
views: [orders, order_checks, restaurants, dining_options]  # include base_view
default_filters:
  - field: "orders.is_voided"
    eq:
      value: false
```

Shape: `name`, `description`, `base_view`, `views`, `default_filters`. Name
topics after the questions people ask; a topic usually spans several views
("one topic per view" is not the pattern). Let `default_filters` carry rules
nobody should have to remember. Operators: `eq`/`neq`/`gt`/`gte`/`lt`/`lte`
(`value:`), `in`/`not_in` (`values:`), `in_date_range`/`not_in_date_range`
(`from:`/`to:`). **Qualify `field:` as `view.dimension`** — a bare field
resolves against a view *named like the topic*, not `base_view`.

**Twin views** (e.g. a ClickHouse → Airhouse migration leaving `sales_daily`
and `airhouse_sales_daily`) are not mirrors: their measures diverge, and
their entity names differ (`airhouse_restaurant` vs `restaurant`) so a topic
mixing families silently fails to join. Keep each topic within one family,
read the twin before assuming a measure exists, and don't copy the pattern.

## Pre-aggregations (rollups)

A rollup is a materialized partial aggregate of **one** view; a fully covered
query is answered from it instead of scanning the warehouse. **Opt-in per
view — no `pre_aggregations:` block means no rollups at all** (no implicit
default).

```yaml
refresh_key:                     # view-level default for every rollup below
  every: "1h"
pre_aggregations:
  - name: orders_by_month
    dimensions: [order_status]   # names declared in THIS view
    measures: [total_orders, total_order_value]
    time_dimension: order_date   # a date/datetime dimension of this view
    granularity: month           # second|minute|hour|day|week|month|quarter|year
  - name: orders_summary
    measures: [total_orders]
    refresh_key:                 # per-rollup override
      sql: "SELECT MAX(updated_at) FROM orders"
```

- `refresh_key` takes **exactly one** of `every` or `sql` — both, neither or
  another key is a parse error. `sql` re-runs every cycle; keep it a cheap
  `MAX`.
- A rollup naming a member the view doesn't declare is **refused**.
- A rollup serves only when: every requested and filtered dimension is in its
  `dimensions` (a filter on `time_dimension` is fine); every measure is listed
  **and** re-aggregatable; the requested granularity is the stored one or
  coarser — **except `week`**, which serves neither `month`, `quarter` nor
  `year` (store `day` for both). Otherwise the warehouse answers — correct,
  just slower.
- Limits: one view's members only; fixed granularities. Multi-view or custom
  buckets need a rollup table built by an automation.

`config.yml` (all optional): `pre_aggregations: { schema: AIRLAYER, database:
<connector, default each view's datasource>, refresh_worker: { enabled: true,
heartbeat: "10m" (clamped 30s–24h), renewal_threshold: "120s" (clamped
10s–1h) } }`.

## Validating a change

1. **`validate_project` / `oxy validate --file <view-or-topic>`** on every
   file you touch — catches a bare literal in `samples`, a missing
   `entities:`/`dimensions:` list, a removed feature. It does **not** look
   inside `pre_aggregations:` or `refresh_key:`.
2. **Run a real query through the topic** with the builder's `semantic_query`
   tool before calling it done. Validators don't check cross-file wiring — an
   entity join that matches nothing, a topic naming a missing view, a measure
   a twin lacks.
3. **`oxy build` does not validate the semantic model** (search embeddings
   only). Read real column names from `.databases/` (written
   by `oxy sync`).

When refreshing a view, build fresh from the schema source (`.databases/`),
not from git history — old definitions drift from the live schema.

| Error | Fix |
| --- | --- |
| "measure not found" after switching topics | the twin view lacks it — read the twin |
| topic returns missing rows / no join | entity names differ between views, or the topic mixes twin families |
| `TYPE_MISMATCH` on a date filter | sample one row; `type: date` + cast from the table above |
| "unresolved reference" | a `{{ }}` names no dimension/measure |
| whole model fails to load | a bare boolean/number in `samples`, or a bare string in `meta` |
| rollup never serves | missing dimension, filtered dimension not listed, non-re-aggregatable measure, or `week` → month |
