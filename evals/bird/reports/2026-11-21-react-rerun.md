# BIRD Mini-Dev: react re-run on the PR B build (2026-11-21)

**React answers 83.3% (135/162) again, the same total as the 2026-11-20 run.** PR B (#4269)
changed the react prompt (the data-freshness hint) and the served path; this re-run checks the
headless eval did not move. Ten questions changed verdict, five each way, which is the
single-sample noise the 11-20 report expects. The suite cost $16.83 (11-20: $18.69), median
13.6 s per question (11-20: 14.3 s).

| db | 11-20 | 11-21 |
|---|---|---|
| financial | 40/46 | 40/46 |
| debit_card_specializing | 8/13 | 7/13 |
| california_schools | 38/48 | 39/48 |
| formula_1 | 49/55 | 49/55 |
| **pooled** | **135/162** | **135/162** |

Changed verdicts (by case index within each suite):

- CORRECT on 11-21, INCORRECT on 11-20: california_schools 6, 32, 41; financial 45; formula_1 39.
- INCORRECT on 11-21, CORRECT on 11-20: california_schools 33, 40; debit_card_specializing 8; financial 39; formula_1 5.

## Run notes

- **Build.** `target/debug/oxy` built from main at `b5937c977` (#4269 merged). Every graded
  record names `runtime: react` and `model: claude-opus-4-8`.
- **Run.** `cd evals/bird && BIRD_PG_PASSWORD=bird_ro ../../target/debug/oxy test
  <db>.react.test.yml --output-json`, the four suites one after another. Wall: financial
  314 s, debit_card_specializing 50 s, california_schools 397 s, formula_1 228 s.
- **One sample per question**, judged by `claude-opus-4-8` (the eval model), as before. The
  FSM arm is still the 2026-11-18 baseline (72/162), not re-run.
- **Skipped:** debit_card_specializing 149, as in every run (the gold SQL fails under the
  connector's wrapping).
- **Results** (git-ignored) are archived in
  `~/Workspace/oxy-evals-archive/bird-react-2026-11-21-prb/` with the per-db logs.

## Against the FSM baseline

Generated with:

```bash
python3 evals/bird/bird.py compare \
  --fsm-dir ~/Workspace/oxy-evals-archive/bird-fsm-baseline-2026-11-18 \
  --react-dir ~/Workspace/oxy-evals-archive/bird-react-2026-11-21-prb \
  --model claude-opus-4-8 --price-in 5 --price-out 25
```

### financial

| metric | fsm | react |
|---|---|---|
| answer accuracy | 39.1% (18/46) | 87.0% (40/46) |
| EX | 10.9% | 41.3% |
| Soft-F1 | 21.6% | 63.3% |
| median latency s | 41.3 | 18.2 |
| median turns | 6.0 | 4.0 |
| tokens in / out | 1,545,414 / 223,607 | 348 / 82,899 |
| cost | $24.90 | $5.08 |

### debit_card_specializing

| metric | fsm | react |
|---|---|---|
| answer accuracy | 15.4% (2/13) | 53.8% (7/13) |
| EX | 0.0% | 30.8% |
| Soft-F1 | 7.7% | 46.5% |
| median latency s | 36.1 | 9.3 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 170,309 / 30,242 | 96 / 9,205 |
| cost | $3.07 | $0.65 |

### california_schools

| metric | fsm | react |
|---|---|---|
| answer accuracy | 31.2% (15/48) | 81.2% (39/48) |
| EX | 10.4% | 27.1% |
| Soft-F1 | 21.8% | 60.1% |
| median latency s | 38.3 | 17.2 |
| median turns | 6.0 | 4.0 |
| tokens in / out | 3,604,487 / 501,286 | 378 / 74,334 |
| cost | $55.66 | $7.16 |

### formula_1

| metric | fsm | react |
|---|---|---|
| answer accuracy | 67.3% (37/55) | 89.1% (49/55) |
| EX | 16.4% | 20.0% |
| Soft-F1 | 36.2% | 47.9% |
| median latency s | 34.2 | 11.5 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 2,149,336 / 215,097 | 400 / 46,361 |
| cost | $34.46 | $3.94 |

### pooled

| metric | fsm | react |
|---|---|---|
| answer accuracy | 44.4% (72/162) | 83.3% (135/162) |
| EX | 11.7% | 29.0% |
| Soft-F1 | 25.5% | 55.8% |
| median latency s | 36.6 | 13.6 |
| median turns | 6.0 | 3.0 |
| tokens in / out | 7,469,546 / 970,232 | 1,222 / 212,799 |
| cost | $118.09 | $16.83 |

