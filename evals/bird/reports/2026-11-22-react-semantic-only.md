# BIRD Mini-Dev: react on the semantic model only (2026-11-22)

**React answers 85.2% (138/162) using the semantic model only. The FSM answers 44.4% (72/162),
on that same language.** So the FSM's failures mostly do not come back when react drives the
semantic model. Free react (raw SQL allowed, 11-21) answered 83.3% (135/162). Of the 66 questions react won and the FSM
lost on 11-20, semantic-only react keeps 59.

The open cost is the query interface, not what airlayer can express. 271 of 652 `query` calls (42%)
failed, and 237 of those were malformed `semantic` arguments: `=` or `equals` for `eq`,
`is_not_null`, `value` and `values` swapped. Each wrong call is a wasted turn. Three such loops
hit the $3 per-question cap.

| arm | answer accuracy | median s | cost |
|---|---|---|---|
| FSM (11-18) | 44.4% (72/162) | 36.6 | $118.09 |
| react, raw SQL allowed (11-21) | 83.3% (135/162) | 13.6 | $16.83 |
| react, `semantic_only` (this run) | 85.2% (138/162) | 24.7 | $28.10 |

## Do the FSM's failure modes come back?

The 66 react-only wins of 11-20, by the FSM failure category of
`fsm-failure-analysis` (the category is the FSM's failure, not this run's):

| FSM failure category | kept by semantic-only react | lost |
|---|---|---|
| Shape ceiling: a nearby, expressible question answered | 20/23 | fin 219 (budget), cal 42 (9 of 929 rows), f1 251 |
| Wrong member, filter or sort default | 6/9 | fin 235 (budget), cal 4, cal 26 |
| Prose math over a 50-row preview | 7/8 | dcs 157 (rounding) |
| Stopped: a question or budget ends the run | 7/7 | |
| Fan-out CTE drops the measure's filters | 7/7 | |
| Literal format mismatch | 7/7 | |
| Refused: the structured query can't express it | 5/5 | |
| **total** | **59/66** | |

- **The two FSM-design categories don't come back** ("stopped", "prose math", plus the literal-format
  retries): react runs to an answer, reads the full result and retries a zero-row filter.
- **The airlayer-language categories mostly don't come back either.** "Shape ceiling" (23) and
  "refused" (5) were questions the FSM's one structured query could not express. React answers
  25 of those 28 with the same language, by chaining several semantic queries and computing the
  last step from their results.
- **The fan-out bug class is fixed upstream:** this is the first arm on airlayer `b066f54`
  (#142). The FSM ran on the old airlayer, so its fan-out losses would differ today; it was
  not re-run.
- Semantic-only react loses only one question the FSM got right (cal 5: a malformed filter, then a
  wrong count).

## Against free react

Against the 11-21 free run, semantic-only wins nine questions and loses six. That is within the
single-sample noise the earlier reports measured (10 flips between two free runs). Five of the six
losses are budget or interface failures:

- **fin 219 and fin 235:** run failures at the $3 cap.
- **cal 4:** a misspelled member, then a wrong address.
- **cal 5 and cal 26:** malformed filters, then a wrong number.
- **f1 251:** a wrong reading of the question.

## Where the remaining 24 failures come from

- **Run failures (3):** fin 219, fin 235 and cal 41 hit the $3 per-question cost cap after
  malformed-query loops. They cost about $9.40 of the $28.10, and they are why the run cost
  more than the ~$17 estimate.
- **Incomplete lists (5):** fin 198, cal 1, cal 8, cal 42 and cal 46 have partial rows; cal 42
  has 929 gold rows.
- **Off-by-small counts or sums (7):** most are on debit_card_specializing (154, 156, 157, 158,
  162) and on fin 204 and 236. These look like gold-definition or rounding differences,
  as in every earlier run on this db.
- **Wrong reading of the question (9):** fin 225 (rank direction) and f1 251, 256, 267, 293;
  plus cal 4, 5, 24 and 26, which follow malformed-filter retries.

## What this says about react with semantic models

- **The FSM's failures are mostly not airlayer's ceiling:** with an agent that can take several
  steps, the same language answers 85%.
- **The next lever is the tool's interface.** `query`'s `semantic` parameter is described in one
  line. A JSON schema with the filter `op` enum and the `value` / `values` rule would remove most of
  the 237 malformed calls, and with them most of the turns, cost and three budget failures.
  Measure it with this arm.

## How it ran

- **Build:** PR #4314 at `53f5d48c6`. `<db>.semantic.agentic.yml` is the react file plus
  `semantic_only: true`. Every query and every judged answer was `semantic`: 378 successful
  executions, 0 raw. Three answers have no source because their runs failed.
- **Samples:** one per question; the judge and eval model are `claude-opus-4-8`. One question
  was skipped for a gold-SQL error (dcs 149), as before.
- **Results** (git-ignored) are archived in
  `~/Workspace/oxy-evals-archive/bird-semantic-2026-11-22/`, with the per-db logs. Each record's
  `timing.tool_errors` holds the failed calls' text.

```bash
python3 evals/bird/bird.py report --runtime semantic \
  --dir ~/Workspace/oxy-evals-archive/bird-semantic-2026-11-22 \
  --model claude-opus-4-8 --price-in 5 --price-out 25
python3 evals/bird/bird.py compare --react-arm semantic \
  --fsm-dir ~/Workspace/oxy-evals-archive/bird-fsm-baseline-2026-11-18 \
  --react-dir ~/Workspace/oxy-evals-archive/bird-semantic-2026-11-22 \
  --model claude-opus-4-8 --price-in 5 --price-out 25
```
