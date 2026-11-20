# BIRD Mini-Dev data loader

Design: `internal-docs/agent-engine-runtime.md` (section "Eval suite: BIRD Mini-Dev subset").
Latest report: `evals/bird/reports/2026-11-18-fsm-baseline.md`.

Fetches BIRD Mini-Dev, splits its single Postgres dump into one database per BIRD
DB, and loads four of them into a local Docker Postgres. Python 3.11+, stdlib only.

Downloaded data is cached in `~/.cache/oxy-evals/bird/` and is never committed
(BIRD is CC BY-SA 4.0).

```bash
python3 evals/bird/bird.py fetch          # ~800 MB zip + question jsonl, size and sha256 checked
python3 evals/bird/bird.py load           # container oxy-bird-pg, port 15433; --fresh re-creates DBs
python3 evals/bird/bird.py smoke          # as bird_ro: row counts, DELETE denied, gold SQL runs
```

- Databases: `bird_financial`, `bird_debit_card_specializing`, `bird_california_schools`,
  `bird_formula_1`. Superuser `postgres`/`postgres`; read-only role `bird_ro`/`bird_ro`
  (read-only: CONNECT, TEMP, SELECT; `statement_timeout` 60 s, set by `load`). Port 15433 avoids `oxy start`'s own Postgres on 15432.
- `smoke` tolerates only the cross-dialect gold-SQL failures the Hugging Face manifest
  lists (question ids 132, 172, 175, 177, 184, 185, 216).

## Unit tests

From the repo root:

```bash
python3 -m unittest discover -s evals/bird/tests -t evals/bird
```

## Eval workspace

`evals/bird/` is itself an Oxy workspace: `config.yml` declares the four
`bird_<db>` Postgres databases (read-only role `bird_ro`, loopback port 15433) and one
model, `eval-model`. Each `<db>.agentic.yml` is an analytics agent on that database with
`llm.ref: eval-model` and no FSM overrides; semantic models go in `evals/bird/<db>/`.

```bash
export BIRD_PG_PASSWORD=bird_ro
export ANTHROPIC_API_KEY=...
python3 evals/bird/bird.py gen-cases            # rewrites <db>.test.yml (1 run per case)
cd evals/bird && ../../target/debug/oxy test financial.test.yml --case 0 --format json
```

- `eval-model` is `claude-opus-4-8`, which is `agentic_llm::constants::DEFAULT_MODEL`: what an
  analytics agent with no `llm.ref` runs on today. `eval-model` in `config.yml` is the one place
  to change it.
- The `<db>.test.yml` files are generated and committed; do not edit them. Cases listed in
  `disputed.json` (question ids) are left out. A milestone run regenerates with
  `gen-cases --runs 3` and does not commit that.
- BIRD's `evidence` field is never written into a test file: the agent gets the bare question.

## The react arm and the comparison

`<db>.react.agentic.yml` is its FSM file plus `runtime: react` (a test checks that nothing else
differs). `gen-cases --runtime react` writes only `<db>.react.test.yml` (suite `bird-react-<db>`,
same cases); the FSM files stay as they are. Run it the same way, from `evals/bird`:

```bash
../../target/debug/oxy test financial.react.test.yml --output-json   # -> financial.react.test.results.json
python3 evals/bird/bird.py report --runtime react --dir <dir> --model ... --price-in ... --price-out ...
python3 evals/bird/bird.py compare --fsm-dir <dir> --react-dir <dir> --model ... --price-in ... --price-out ...
```

- Every record carries `runtime` (`react` once the run's `RuntimeSelected` event is seen, else
  `fsm`). A record without the field is `fsm`, which is how the archived baseline loads.
  `load_results` raises when a record disagrees with the requested runtime; a run that failed
  before answering records none and is exempt.
- A react run's answer SQL is its `AnswerResult` (the result the answer cites), which arrives
  after the core `Done`; it pins `final_sql`. The FSM keeps the last successful query.
- Turns and tokens: "turns" is the LLM calls per question (one `LlmStart` each) for both runtimes:
  an FSM question runs several states and tool rounds, a react question one call per loop turn.
  Median turns is over runs that produced a measurement. Output and cache tokens come from
  `LlmEnd` in both. Input: a react `LlmEnd` carries the provider's uncached input (`input_tokens`),
  and the react `LlmStart` estimate is not counted; the FSM reports prompt size on `LlmStart`
  (an estimate on the first round, then the previous round's provider uncached input plus its
  output plus an estimate for the tool results), so the two inputs are close, not identical in
  meaning. The react `LlmEnd.duration_ms` is the model call's wall time (retry backoff included), so the
  Timing section works; TTFT shows `n/a (react)` when no round streamed a token.
- Each record carries the `model` its LLM calls reported. `report` and `compare` print it, the
  judge model, and the `thinking:` keys in the agent files and `config.yml` (none set means the
  solving state's default, adaptive; read from this checkout, not from the run). `report` and
  `compare` refuse when any recorded model differs from `--model`; `compare` also refuses when the
  arms' recorded models differ. Archived records have none and read `unrecorded`, which passes
  (it cannot be checked) and is printed.
- `compare` prints per-db and pooled tables (answer accuracy, EX, Soft-F1, median latency,
  median turns, tokens, cost) and the question ids whose judge verdicts differ.

## Regrade without re-running the agent

A change to the grader (judge prompt, gold-SQL timeout, SKIP rule) is applied to an execution-graded
existing run by grading its recorded outputs again. Only the gold SQL, the agent's final SQL
and the judge run; the cost is the judge calls (about 160 for the full suite), not the agent's.
Results files are overwritten by `--output-json`, so regrade from a copy:

```bash
cd evals/bird && ../../target/debug/oxy test financial.test.yml \
  --regrade ~/somewhere/financial.test.results.json --output-json
```
