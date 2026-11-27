# Writing `expected` answers

The judge PASSes an answer that contains the **core factual content** of `expected`, with no
contradiction and no significant omission. It ignores formatting, style, phrasing and level of
detail, and it allows extra correct information. Every word in `expected` is therefore a fact
the agent must state. Write only the facts that must hold, as the answer a careful analyst
would give.

## By question type

**Single aggregate**

```text
Total revenue across all locations in 2025 is approximately $6.7 billion.
```

**Ranking.** Give the criterion once, the top entries with anchor values, and the bottom of the
list if it matters.

```text
The top 5 stores by 2025 net sales are led by Store 20 (~$301M) and Store 4 (~$299M);
the fifth, Store 13, is at ~$287M.
```

**Trend.** Give the direction, the current value, and one or two notable points.

```text
Weekly orders in Q4 2025 rose from ~11K in early October to a peak of ~19K in the
week of 24 November, then settled near ~14K in December.
```

**Comparison between groups**

```text
In 2025, stores in the West region averaged ~$1.2M monthly sales, about 15% above the
~$1.04M average of the other regions.
```

**Share or ratio.** Say what the denominator is.

```text
Returns were ~4.2% of units sold in 2025 (about 52K of 1.24M units).
```

**Filter on a rare value.** Assert the scope, so an answer that ignores the filter fails.

```text
Only 3 orders in 2025 used the GIFTCARD50 promotion, totalling ~$410.
```

**An empty result that is the correct answer**

```text
There were no refunds issued on 25 December 2025.
```

**Unanswerable as asked.** Give the reason, and what a good answer offers instead.

```text
Margin by product can't be computed because cost data is only recorded at the category
level. A correct answer explains this and offers margin by category instead.
```

## Evidence rules

- **Trust facts that agree across runs over any single run.** Two or more agreeing runs are
  the minimum for a full `expected`.
- **A "successful" run can still be a failure.** Treat these as non-answers, not evidence:
  - an answer with no figures where figures were asked for
  - a ranked list whose metric values are all blank or null
  - an answer built on a row-capped or truncated result, when the question needed every row
- **Separate transient errors from systematic ones.** A connection reset or a timeout can
  vanish on a re-run. The same error on every run is a finding to report, not noise.
- **Verify before you encode.** If you can query the data yourself, cross-check the headline
  number. A stable answer can be stably wrong: a wrong join, a double-counted fact table, the
  wrong date column. Encoding it turns a bug into the specification.
- **When evidence is thin, narrow the placeholder instead of guessing:**

```yaml
expected: >-
  DRAFT: all 3 runs agree the West region leads; the size of the lead varied
  (12%–31%) — needs a verified figure.
```

## Relative time

If the prompt says "last month" and the runs show the agent resolved it to November 2025,
rewrite **both** sides: the prompt becomes "…in November 2025?", and `expected` uses the same
period. Leaving the prompt relative makes the test start failing next month. The agent
resolves relative dates against today in its `timezone:` (UTC when unset).

## When to split, rewrite, or flag a case

- **Split** a prompt that asks two things. The judge fails the whole run if either half is
  missing, so you can't tell which half broke.
- **Propose a rewrite** when the runs answer different, reasonable readings of the prompt
  (net or gross, calendar or fiscal year), or when the agent stops to ask a clarifying
  question. That ends the run as an error (unless the case sets `expect: {asks_user: true}`,
  to test that the agent asks). Show the user the readings you saw, and let them choose.
- **Flag, don't draft,** when no view or table holds the data. That's a gap in the agent, not
  something a test can fix.

## Checklist before saving

- [ ] Every case has a `name:` slug.
- [ ] No relative dates remain in prompts that `expected` depends on.
- [ ] Numbers are approximate unless they're exact and stable.
- [ ] No schema, view or column names the user wouldn't say.
- [ ] No `expected` was copied from a single run without agreement or verification.
- [ ] Uncertain cases carry a narrowed `DRAFT:` note, not an invented value.
- [ ] The file parses: once it is pushed, run it by path
      (`oxyc evals run <file> --workspace <id> --case 0`). A run with no file skips a broken
      file silently.
