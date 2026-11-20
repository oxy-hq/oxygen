# BIRD Mini-Dev: react runtime vs the FSM baseline (2026-11-20)

**React answers 83.3% (135/162) of graded questions correctly; the FSM baseline answers 44.4%
(72/162).** The ship bar was react ≥ FSM, so it clears it by 38.9 points. React answers are
correct on 66 questions where the FSM's are wrong, and wrong on 3 where the FSM's are correct.
It takes a median 14.3 s per question (the FSM 36.6 s) and costs $18.69 for the suite (the FSM
$118.09).

Read the numbers with the caveats under *Run notes*: one sample per question, a judge that is
the eval model itself, and an FSM arm that was run on 2026-11-18 and not re-run.

Generated with:

```bash
python3 evals/bird/bird.py compare \
  --fsm-dir ~/Workspace/oxy-evals-archive/bird-fsm-baseline-2026-11-18 \
  --react-dir ~/Workspace/oxy-evals-archive/bird-react-2026-11-20-full \
  --model claude-opus-4-8 --price-in 5 --price-out 25
```


Eval model: `claude-opus-4-8` for both arms ($5.0/M input, $25.0/M output). Judge model: `claude-opus-4-8` (fsm), `claude-opus-4-8` (react).

- fsm: Recorded model: `unrecorded`. Thinking: solving state: default (adaptive): no `thinking:` key set (from the agent files in this checkout, not from the run).
- react: Recorded model: `claude-opus-4-8`. Thinking: solving state: default (adaptive): no `thinking:` key set (from the agent files in this checkout, not from the run).

Runs that failed before answering carry no usage: they are excluded from the median latency and turns, and their tokens and cost are missing, so cost is a lower bound on that count.

Median turns is the median LLM calls per question (one `LlmStart` each) for both runtimes, over runs that produced a measurement. Token counts read the same event fields for both arms. Output and cache tokens are the provider's in both. Input tokens are the provider's uncached input for react; the FSM reports its prompt size on `LlmStart`: an estimate on the first round, then the previous round's provider uncached input plus its output plus an estimate for the tool results, so input is close but not identical in meaning.

### financial

| metric | fsm | react |
|---|---|---|
| answer accuracy | 39.1% (18/46) | 87.0% (40/46) |
| EX | 10.9% | 39.1% |
| Soft-F1 | 21.6% | 64.3% |
| median latency s | 41.3 | 16.7 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 1,545,414 / 223,607 | 370 / 90,479 |
| cost | $24.90 | $5.39 |

### debit_card_specializing

| metric | fsm | react |
|---|---|---|
| answer accuracy | 15.4% (2/13) | 61.5% (8/13) |
| EX | 0.0% | 23.1% |
| Soft-F1 | 7.7% | 40.8% |
| median latency s | 36.1 | 18.1 |
| median turns | 6.0 | 4.0 |
| tokens in / out | 170,309 / 30,242 | 204 / 44,158 |
| cost | $3.07 | $2.07 |

### california_schools

| metric | fsm | react |
|---|---|---|
| answer accuracy | 31.2% (15/48) | 79.2% (38/48) |
| EX | 10.4% | 33.3% |
| Soft-F1 | 21.8% | 58.2% |
| median latency s | 38.3 | 17.3 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 3,604,487 / 501,286 | 396 / 68,843 |
| cost | $55.66 | $7.07 |

### formula_1

| metric | fsm | react |
|---|---|---|
| answer accuracy | 67.3% (37/55) | 89.1% (49/55) |
| EX | 16.4% | 21.8% |
| Soft-F1 | 36.2% | 49.9% |
| median latency s | 34.2 | 12.1 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 2,149,336 / 215,097 | 432 / 50,771 |
| cost | $34.46 | $4.16 |

### pooled

| metric | fsm | react |
|---|---|---|
| answer accuracy | 44.4% (72/162) | 83.3% (135/162) |
| EX | 11.7% | 30.2% |
| Soft-F1 | 25.5% | 55.7% |
| median latency s | 36.6 | 14.3 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 7,469,546 / 970,232 | 1,402 / 254,251 |
| cost | $118.09 | $18.69 |

## Questions with differing verdicts (69)

- california_schools 0: fsm INCORRECT, react CORRECT
- california_schools 2: fsm INCORRECT, react CORRECT
- california_schools 4: fsm INCORRECT, react CORRECT
- california_schools 7: fsm CORRECT, react INCORRECT
- california_schools 10: fsm INCORRECT, react CORRECT
- california_schools 11: fsm INCORRECT, react CORRECT
- california_schools 13: fsm INCORRECT, react CORRECT
- california_schools 14: fsm INCORRECT, react CORRECT
- california_schools 15: fsm INCORRECT, react CORRECT
- california_schools 16: fsm INCORRECT, react CORRECT
- california_schools 17: fsm INCORRECT, react CORRECT
- california_schools 19: fsm INCORRECT, react CORRECT
- california_schools 20: fsm INCORRECT, react CORRECT
- california_schools 21: fsm INCORRECT, react CORRECT
- california_schools 22: fsm INCORRECT, react CORRECT
- california_schools 25: fsm INCORRECT, react CORRECT
- california_schools 26: fsm INCORRECT, react CORRECT
- california_schools 30: fsm INCORRECT, react CORRECT
- california_schools 31: fsm INCORRECT, react CORRECT
- california_schools 34: fsm CORRECT, react INCORRECT
- california_schools 35: fsm INCORRECT, react CORRECT
- california_schools 37: fsm INCORRECT, react CORRECT
- california_schools 38: fsm INCORRECT, react CORRECT
- california_schools 40: fsm INCORRECT, react CORRECT
- california_schools 42: fsm INCORRECT, react CORRECT
- california_schools 44: fsm INCORRECT, react CORRECT
- california_schools 48: fsm INCORRECT, react CORRECT
- debit_card_specializing 151: fsm INCORRECT, react CORRECT
- debit_card_specializing 152: fsm INCORRECT, react CORRECT
- debit_card_specializing 153: fsm INCORRECT, react CORRECT
- debit_card_specializing 155: fsm INCORRECT, react CORRECT
- debit_card_specializing 157: fsm INCORRECT, react CORRECT
- debit_card_specializing 161: fsm INCORRECT, react CORRECT
- financial 194: fsm INCORRECT, react CORRECT
- financial 197: fsm INCORRECT, react CORRECT
- financial 199: fsm CORRECT, react INCORRECT
- financial 205: fsm INCORRECT, react CORRECT
- financial 211: fsm INCORRECT, react CORRECT
- financial 212: fsm INCORRECT, react CORRECT
- financial 213: fsm INCORRECT, react CORRECT
- financial 214: fsm INCORRECT, react CORRECT
- financial 215: fsm INCORRECT, react CORRECT
- financial 216: fsm INCORRECT, react CORRECT
- financial 217: fsm INCORRECT, react CORRECT
- financial 219: fsm INCORRECT, react CORRECT
- financial 220: fsm INCORRECT, react CORRECT
- financial 221: fsm INCORRECT, react CORRECT
- financial 222: fsm INCORRECT, react CORRECT
- financial 223: fsm INCORRECT, react CORRECT
- financial 224: fsm INCORRECT, react CORRECT
- financial 227: fsm INCORRECT, react CORRECT
- financial 229: fsm INCORRECT, react CORRECT
- financial 230: fsm INCORRECT, react CORRECT
- financial 231: fsm INCORRECT, react CORRECT
- financial 233: fsm INCORRECT, react CORRECT
- financial 234: fsm INCORRECT, react CORRECT
- financial 235: fsm INCORRECT, react CORRECT
- formula_1 243: fsm INCORRECT, react CORRECT
- formula_1 244: fsm INCORRECT, react CORRECT
- formula_1 246: fsm INCORRECT, react CORRECT
- formula_1 251: fsm INCORRECT, react CORRECT
- formula_1 252: fsm INCORRECT, react CORRECT
- formula_1 264: fsm INCORRECT, react CORRECT
- formula_1 265: fsm INCORRECT, react CORRECT
- formula_1 272: fsm INCORRECT, react CORRECT
- formula_1 275: fsm INCORRECT, react CORRECT
- formula_1 276: fsm INCORRECT, react CORRECT
- formula_1 290: fsm INCORRECT, react CORRECT
- formula_1 291: fsm INCORRECT, react CORRECT

## Run notes

- **Arms.** FSM: the 2026-11-18 baseline (`evals/bird/reports/2026-11-18-fsm-baseline.md`),
  not re-run. React: `<db>.react.test.yml` (the same 163 cases, the agent file plus
  `runtime: react`), run 2026-11-20 with `target/debug/oxy` built from main after #4255 and
  #4256. Run: `cd evals/bird && BIRD_PG_PASSWORD=bird_ro ../../target/debug/oxy test
  <db>.react.test.yml --output-json`, the four suites one after another. Wall per db:
  financial 362 s, debit_card_specializing 197 s, california_schools 381 s, formula_1 251 s
  (about 20 min in all; the FSM took about 98 min).
- **One sample per question** (`runs: 1`) in both arms. A few questions would flip on a
  re-run; a gap of 63 questions would not.
- **Judge.** Both arms are graded by `claude-opus-4-8`, which is also the eval model, so the
  judge is not independent of the agent. The judge grades the answer text, the same way in both arms.
- **What react could read.** `search` and `read` reach only the agent's `context:` globs,
  `./<db>/*.view.yml` and `./<db>/*.topic.yml`: the same semantic model the FSM reads. No
  `.md`, `.sql` or `.test.` file matches them, so the gold SQL, the coverage notes and the
  FSM's results were out of reach.
- **Run failures.** React: 1 of 163 (california_schools: the agent called `ask_user` to ask
  whether "K-12 schools" meant the school type or the meal counts; a headless eval run cannot
  answer, so it counts as INCORRECT). FSM: 12, also counted as INCORRECT. Without them the FSM
  is 72/150 = 48.0%, so the failures do not explain the gap.
- **Skipped.** 1 in both arms (debit_card_specializing 149: the gold SQL fails with `42702
  column reference "round" is ambiguous` under the connector's wrapping, as in the baseline).
- **EX and Soft-F1 understate react.** A react run's `final_sql` is the result its answer
  cites. When the model combines two queries in the answer, or computes a figure in prose
  from one result, that cited result does not match the gold rows, though the answer does
  (financial 194: judge CORRECT, EX 0). The judge's answer accuracy is the ship metric.
- **Tokens.** React's input column is the provider's *uncached* input (1,402 in all), since
  almost all of its prompt is served from the prompt cache. The FSM's input column is its
  prompt-size estimate. Cost prices cache reads at $0.50 and cache writes at $6.25 per 1M
  tokens (0.1x and 1.25x input, 5-minute TTL) in both arms, so the cost columns compare; the
  token columns do not. Judge calls are in neither.
- **Turn counts.** React max 19 LLM calls per question; the FSM max 819 (the runaway
  `specifying` loops in the baseline).
- **Not committed:** the results files (`*.react.test.results.json`, git-ignored). They are in
  `~/Workspace/oxy-evals-archive/bird-react-2026-11-20-full/` with the per-db logs.
