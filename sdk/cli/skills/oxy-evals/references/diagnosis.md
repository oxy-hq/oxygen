# Diagnosing and repairing a failing eval

Diagnose before editing. For every failing or flaky case, collect:

1. Every run's `actual_output`, `cot` and `choice` from the results JSON
   ([results-json.md](results-json.md)).
2. The route the agent took, from `oxy agentic run <agent> -q "<prompt>" --json`. Run it two
   or three times if the case is flaky. Read `query_executed.source` and `query`, and any
   `analytics_validation_failed` events.
3. The files the agent actually loads: the `context:` globs in its `.agentic.yml`, resolved
   relative to that file's directory. A view, doc or SQL file outside those globs doesn't
   exist for this agent.

Then name one category below. If two apply, fix the earlier one first. Most failures are
**A** or **B**.

## A. The semantic model lacks a grouping or dimension

**Evidence:** several queries per run, `source: llm` with long hand-written SQL, raw
row-level results, the answer doing arithmetic or bucketing in prose, and runs that disagree
because each one improvises the grouping differently.

**Fix:** add a dimension that names the grouping, so one semantic query returns a small,
finished result.

```yaml
dimensions:
  - name: order_size_band
    type: string
    description: >-
      Order size band: Small (under $50), Medium ($50–$200), Large (over $200). Use to compare
      small, medium and large orders or baskets.
    expr: |
      CASE
        WHEN order_total < 50 THEN 'Small'
        WHEN order_total <= 200 THEN 'Medium'
        ELSE 'Large'
      END
    samples: ["Small", "Medium", "Large"]
```

The thresholds must be **business definitions**, not numbers read off the current data. A
cut-off derived from today's average drifts as the data moves, and a cut-off derived from the
test's expected answer is hard-coding the answer. If the same grouping makes sense in several
views, add it to each of them.

## B. A measure is missing, wrong, or can't be found

**Missing.** The answer computes a ratio, rate or average itself, and the arithmetic varies
across runs. Add the measure:

```yaml
measures:
  - name: avg_order_value
    type: number            # `number`: the expression already aggregates
    description: >-
      Average order value (AOV): net revenue per order. Also called average basket or
      average ticket.
    expr: "SUM(net_revenue) / NULLIF(COUNT(DISTINCT order_id), 0)"
```

**Wrong.** The runs agree on a number that is consistently wrong. Check the measure's
aggregation (`sum` where `count_distinct` was meant), a join that fans out and double-counts,
the wrong date column (record-created rather than order date), and the measure's `filters:`.
Fix the definition, not the answer.

**Can't be found.** The measure exists but the agent never uses it (`source: llm`, or "I
don't have that data"). The analytics agent's catalog search matches a measure's `name`, its
`description` and its view's name, and **does not read `synonyms:`**. Put the user's words
(the ones in the failing prompt) into the measure's `description`.

**A dimension that exists but never gets used.** Check three things: its view is matched by
`context:`; the views are linked by entities (a `primary` entity on one, a `foreign` entity
with the same key on the other); and its `description` says what it is in the user's terms.
For view, measure and entity craft, see the `oxy-semantic-modeling` skill.

**Two similar members, chosen differently on each run** (for example `created_date` against
`order_date`). Make the descriptions decisive:

```yaml
  - name: order_date
    type: date
    description: Date the order was placed. Use for sales, revenue and order trends by date.
  - name: created_date
    type: date
    description: When the database row was written. System metadata, not for business questions.
```

## C. Instructions are vague, missing, or misdirect

**Evidence:** the agent picks a different reasonable definition on each run (gross or net,
calendar or fiscal), formats or scopes the answer inconsistently, or ignores a business
convention.

**Fix, by where the knowledge belongs:**

- **A definition or a business fact** goes in a `.md` doc matched by `context:`. Docs are
  injected into the prompts that clarify the question and write the answer.
- **A rule for every step** goes in top-level `instructions:`.
- **A rule for one stage** goes in `states.<state>.instructions`, for one of `clarifying`
  (reading the question), `specifying` (choosing measures and filters), `solving` (writing
  SQL) or `interpreting` (writing the answer).

```yaml
instructions: |
  "Revenue" means net revenue (after refunds and discounts) unless the user says gross.
states:
  clarifying:
    instructions: |
      Read "last quarter" as the most recent completed fiscal quarter; the fiscal year starts
      on 1 February.
  interpreting:
    instructions: State the period and every filter applied in the answer's first sentence.

context:
  - ../semantics/**/*.view.yml
  - ../semantics/**/*.topic.yml
  - ./docs/*.md
```

Keep instructions general and reusable. A sentence that only makes sense for one test case
("the answer to the Q4 question is $1.2M") is hard-coding the test. It's forbidden, and it
fails the next question anyway.

## D. One canonical query should answer the question every time

**Evidence:** an important, frequently asked question whose correct SQL is known and fixed,
but the agent writes different SQL each run.

**Fix:** a Verified Query, a `.sql` file matched by `context:` that carries an `oxy:` comment
block:

```sql
/*
  oxy:
    description: "Current active headcount by department"
    database: warehouse        # optional: a database name from config.yml
*/
SELECT department, COUNT(*) AS active_headcount
FROM hr.employees
WHERE status = 'active'
GROUP BY department
```

When the agent's search matches the file's name or `description`, the SQL runs **verbatim**.
Nothing is substituted into it, so it only fits questions whose answer is exactly that query.
It's no use for "…in a given month". A `.sql` file without the `oxy:` block is only a style
example for the SQL the model writes.

## E. Validation rejects a correct result

**Evidence:** `analytics_validation_failed` events repeating on a result that was right, runs
that end in `max iterations exceeded`, or answers that drift away from the right one after a
retry. Common causes: `non_empty`, when "none" is a correct answer; `outlier_detection` on
naturally spiky data; and `freshness_check` on a source that loads late.

**Fix:** retune the rule. The `validation:` block **replaces** the default set, so list every
rule that should still run. A stage you leave out isn't checked at all, and a misspelled rule
name stops the agent from loading. The 15 defaults:

```yaml
validation:
  rules:
    specified:
      - name: metric_resolves
      - name: join_key_exists
      - name: filter_unambiguous
    solvable:
      - name: sql_syntax
      - name: tables_exist_in_catalog
      - name: spec_tables_present
      - name: column_refs_valid
      - name: timeseries_order_by_check
    solved:
      - name: non_empty
        enabled: false               # "none" is a valid answer for this agent
      - name: truncation_warning
      - name: no_nan_inf
      - name: outlier_detection
        threshold_sigma: 4.0         # looser than the default for spiky daily data
        min_rows: 12
      - name: null_ratio_check
      - name: duplicate_row_check
      - name: freshness_check
```

Loosen only the rule the evidence names. Validation is what stops wrong answers. Don't switch
it off to make a test pass.

## F. Judge false negative (not an agent problem)

**Evidence:** the answer is correct when you read it, the `cot` reasoning says so, and
`choice` is still `FAIL`. Usually the verdict line is missing from the last five lines of the
judge's response, or it reads `PASS or FAIL`, which parses as FAIL.

**Fix:** don't touch the agent. Report it with the `cot` excerpt. Recommend a stronger
`judge_model`: a model that already exists in `config.yml`, preferably from a different family
than the agent's. Changing the test's settings is the user's call.

A false **positive** (an answer that is wrong but PASSed) is the dangerous one. It usually
means `expected` is too vague to contradict anything. Report it, and propose a sharper
`expected` to the user. Don't change it yourself.

## G. The prompt is underspecified

**Evidence:** runs that end in `agentic pipeline asked a clarifying question: …`, or runs
that answer different reasonable readings of the prompt.

**Fix:** report the readings you saw. The test owner chooses between two remedies:

- **Sharpen the prompt.** This is a change to the test, so the user decides.
- **Teach the agent a default reading**, through instructions or a doc, when the business
  wants one (for example, "when no region is named, report all regions"). That's a legitimate
  agent repair.

## H. The data isn't there

**Evidence:** no view or table holds the needed facts, and every run declines or answers
around it.

**Fix:** say so plainly, and name the missing source. Don't add semantic or instruction hacks
that make the agent sound as if it can answer.

## Also check

- **Dates off by a day, or the wrong month.** The agent resolves "today" and relative dates
  in its `timezone:` (UTC when unset). Set it to the business's zone.
- **The right number for the wrong period.** The prompt is relative ("last month") and
  `expected` was written for an earlier month. Report it: the test needs an absolute period.
- **Flaky runs with different `source` values.** Some runs take the semantic model and others
  write SQL. Make the semantic path win: make the measure findable (B), make the dimensions
  reachable (B), or add a Verified Query (D).

## Order of repair

1. Semantic model: measures, dimensions, entities, descriptions.
2. Context: `.md` definitions, Verified Queries, and the `context:` globs themselves.
3. Instructions: global first, then per-state.
4. Validation tuning.

After each change, `oxy validate <edited file>`, push it, and once the deployment has
compiled it re-run the case at least twice
(`oxyc evals run <file> --workspace <id> --case <name> --output-json`), then the whole file.
Evals read the served revision, so an edit counts only once it is pushed and compiled. Stop
after three cycles that don't hold, and report.
