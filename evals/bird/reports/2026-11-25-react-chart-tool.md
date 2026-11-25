# BIRD Mini-Dev: react with and without the `chart` tool (2026-11-25)

**The `chart` tool costs no accuracy: 93.2% (151/162) with it, 91.4% (148/162) without.**
Both arms ran on Sonnet 5.5, on the same day, against the same 162 questions. Nine verdicts
differ, six favouring the chart arm and three favouring main. That is within single-sample
noise: the 11-21 re-run of one build had ten verdicts differ, five each way. So this run shows
no regression. It does not show a gain.

| | main (`c3966f228`, no chart tool) | #4454 (`84691d18e`, chart tool) |
|---|---|---|
| answer accuracy | 91.4% (148/162) | 93.2% (151/162) |
| EX / Soft-F1 | 13.6% / 49.9% | 16.7% / 49.2% |
| agent cost | $6.99 | $7.04 |
| latency mean / median | 14.1 s / 12.7 s | 12.9 s / 10.7 s |
| LLM rounds (mean) | 3.8 | 3.9 |

| db | main | chart | verdicts gained / lost by the chart arm |
|---|---|---|---|
| financial | 41/46 | 41/46 | +3 / -3 |
| debit_card_specializing | 13/13 | 13/13 | 0 / 0 |
| california_schools | 41/48 | 43/48 | +2 / 0 |
| formula_1 | 53/55 | 54/55 | +1 / 0 |

## Run notes

- **Builds.** `oxy-main` at `c3966f228` (the PR's merge base), `oxy-chart` at `84691d18e`.
  A `grep` of the two binaries for the chart tool's description finds it only in `oxy-chart`.
- **Run.** `BIRD_PG_PASSWORD=bird_ro <bin> test <db>.react.test.yml --output-json`, the four
  suites in turn, the chart arm first and then main. Each arm ran on its own copy of
  `evals/bird`. No memory env vars were set.
- **Model.** Eval model `claude-sonnet-5-5` ($2 / $10 per MTok), judge `claude-opus-4-8`. Every
  record names `claude-sonnet-5-5`. This is the first full react raw-SQL run on Sonnet 5.5;
  the 83.3% of 11-20 and 11-21 was on Opus 4.8.
- **Charts drawn.** The records do not list tool calls, so this run cannot count `chart`
  calls. The chart arm made 7 more model calls in total (630 against 623). On BIRD, whose
  questions ask for values rather than pictures, any chart calls were rare.
- **Skipped:** debit_card_specializing 149, as in every run (its gold SQL fails).
- **Results** (git-ignored) are archived in
  `~/Workspace/oxy-evals-archive/bird-chart-ab-2026-11-25/` with the logs, the binaries and
  the run script.

Generated with:

```bash
python3 evals/bird/bird.py report --runtime react --dir <arm dir> \
  --model claude-sonnet-5-5 --price-in 2 --price-out 10
```
