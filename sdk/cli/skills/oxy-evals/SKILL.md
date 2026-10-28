---
name: oxy-evals
description: >-
  Use when writing, running, or acting on evals for an Oxy agent — `.test.yml` files that run an
  `.agentic.yml` agent and score its answers with an LLM judge — and when the agent's answers are
  wrong. Triggers: "add a test", "write a test case", "write evals", "test this agent", "evaluate
  my agent", "bootstrap an eval suite", "fill in expected answers", "run the tests"; and the
  repair side: "tests are failing", "this test is flaky", "the agent's answer is wrong", "the
  answer should be X but the agent says Y", "debug this eval", "make this test pass", or pasted
  `oxy test --output-json` output or a prompt/response pair to diagnose. Drafts expected answers
  from observed runs, then diagnoses failures and repairs the agent's semantic model, context and
  instructions — never the test.
---

# Oxy evals: draft, run, diagnose, repair

An Oxy eval is a `.test.yml` file: a list of prompts, each with the answer a correct agent
gives. `oxy test` runs the target agent on every prompt several times and asks an LLM judge
whether each answer carries the expected facts.

Drafting the tests and repairing the agent are one loop:

1. **Draft** cases and their `expected` answers from evidence.
2. **Run** them and read the results.
3. **Diagnose** each failure and **repair the agent** until it passes reliably.

**The hard rule: the expected answer is the specification. Never weaken a test to make it
pass.** Don't loosen `expected` to match what the agent said, delete or skip a failing case, or
rewrite a prompt to dodge the failure. If a test itself is wrong (a bad expected value, an
ambiguous prompt), say so, show the evidence, and let the user decide. That's a different
change from a repair, and you don't make it silently.

Reference docs (field tables, CI usage):
<https://www.oxygen-hq.com/docs/guide/build/test-evaluate/testing>

## What can be tested

- **Only `.agentic.yml` agents.** `oxy test` rejects any other target, including classic
  `.agent.yml` agents (that runtime was removed). Inline `tests:` blocks inside agent or
  automation files are gone too. A `.test.yml` file is the only entry point.
- **Only the final answer text is judged.** The judge sees the question, `expected`, and the
  agent's answer. It doesn't see the SQL, the tool calls, or the rows.
- **`tool:` on a case is accepted but not enforced.** It has no effect on pass/fail. Don't
  rely on it to assert that the agent used a particular path.

## The `.test.yml` format

```yaml
name: Sales performance          # optional display name
target: agents/sales.agentic.yml # path from the project root; see "target" below

settings:
  runs: 3                        # times each case runs (default 3)
  concurrency: 5                 # agent runs in flight at once (default 5)
  judge_model: judge             # a model `name:` from config.yml (default: first model there)

cases:
  - name: total-revenue-2025     # optional, but always add it: it makes --case usable
    prompt: What was total revenue across all stores in 2025?
    expected: Total revenue across all stores in 2025 was approximately $6.7 billion.
    tags: [revenue, critical]
```

Rules the parser enforces:

- **Unknown keys are rejected at every level** (file, `settings`, each case). A typo such as
  `judge-model:` or `expect:` makes the file fail to parse. Run with an explicit file and you
  get `Failed to deserialize test config …`. **Run `oxy test` with no file and a broken file
  is skipped silently.** It just isn't in the run, with no error. After editing a test file,
  run it by path once.
- **`prompt` and `expected` are required strings.** Everything else is optional.
- **`target`** is resolved from the project root (the directory holding `config.yml`). If you
  omit it, it's inferred from the filename, in the same directory: `sales.agentic.test.yml`
  targets `sales.agentic.yml` beside it. Tests kept in a separate `tests/` folder must set
  `target` explicitly.
- **`judge_model` names a model by its `name:` under `models:` in `config.yml`.** It isn't a
  vendor model ID. If you leave it out, the judge is the **first** model listed in
  `config.yml`. An unknown name fails the run before any judging (`LLM model '<name>' could not
  be resolved from project config`). Google/Gemini models can't be used. Prefer a judge from a
  different model family than the agent's, to avoid self-preference bias.

Naming and placement: `<agent-stem>.<topic>.test.yml` (for example
`sales.performance.test.yml`) in the folder the project already uses for tests. Discovery
(`oxy test` with no file) finds every `*.test.yml` in the project, except under hidden
(dot-prefixed) directories and `target/`, `node_modules/`, `dist/` or `build/`.

## When to write evals

- Before changing an agent, a semantic model, or instructions, so there's a baseline to
  compare against.
- When a user reports a wrong answer. Turn the prompt into a case first, then repair.
- For the questions the business actually asks, and for the edges: a question the agent must
  decline, a ranking, a trend, a filter on a rare value.

## Drafting cases

### Prompts

- **Use absolute periods.** The agent resolves "last month" or "this week" against *today*
  in its `timezone:` (UTC when unset). The expected answer for a relative prompt goes stale
  the next month. Write "November 2025", not "last month".
- **Make prompts specific enough to answer without a follow-up question.** Tests run
  headless. If the agent stops to ask the user something, the run ends as an **error**
  (`agentic pipeline asked a clarifying question: …`), which counts as a FAIL. An ambiguous
  prompt shows up as a run error, not as a wrong answer.
- **Test one thing per case.** "Revenue by region and the top product in each" is two cases.
- **Name every case** (`name:`), with a short stable slug.

### Scaffolding with placeholders

When the right answer isn't known yet, scaffold the cases with a placeholder and fill it in
from evidence:

```yaml
expected: "DRAFT: to be synthesized from repeated runs."
```

A placeholder case's verdict means nothing, because the judge has no facts to check. Never
report a suite as passing while any `DRAFT:` remains in it.

### Writing `expected`

`expected` is what the judge checks the answer against. The judge's instruction is to PASS an
answer that contains the **core factual content** of `expected`. It ignores formatting,
phrasing, and level of detail, and it allows extra correct information. So write the facts
that must be present, and nothing that may legitimately vary.

- State the facts in plain business language, 1–4 sentences. Don't start with "The response
  should…". `expected` is an answer, not a grading instruction.
- **Approximate numbers** (`~`, "about", "roughly") unless an exact value is reliably stable.
  The judge compares facts, and a cent-level total invites false failures.
- **Rankings:** name the criterion once, then the top 2–3 entities with anchor values. Don't
  list every row.
- **Trends:** the direction, the current value, and one or two notable points.
- **Questions the agent can't answer:** state why, and what a correct answer offers instead.
- No internal field, view or schema names unless the user would say them.

```text
Good: Total revenue across all locations in 2025 is approximately $6.7 billion.

Good: The top 5 items by first-week engagements in February 2026: #1 had ~25K engagements
      and #5 had ~10K. Each entry gives the title, channel and engagement count.

Good: Regional plan attainment can't be calculated because targets are set at the company
      level only. A correct answer says so and offers company-level attainment instead.

Bad:  The response should explain total revenue across all stores.      (grading instruction)
Bad:  Total revenue is $6,737,218,004.32.                               (false precision)
Bad:  Sum of store__weekly_sales across all store__store_id values.     (schema names)
Bad:  Store A $73.3M, Store B $275M, Store C $402K, …                   (exhaustive list)
```

More worked examples, and the evidence rules for each kind of question:
[references/writing-expected.md](references/writing-expected.md).

## Running

Run from the project root (the directory holding `config.yml`). The file argument is resolved
from your current directory, and `target` from the project root.

```bash
oxy test tests/sales.performance.test.yml                                  # one file
oxy test tests/sales.performance.test.yml --case total-revenue-2025 --output-json
oxy test tests/sales.performance.test.yml --case 0 --output-json           # by 0-based index
oxy test tests/sales.performance.test.yml --tag critical --output-json     # by tag
oxy test                                                                   # every *.test.yml
oxy test --min-accuracy 0.8 --quiet                                        # CI gate, exit 1 below
```

- `--case` takes a 0-based index, a case `name`, or the exact prompt. It requires a file. With
  `--tag` as well, a case must match both. If two cases share a name, the first one runs.
- `--tag` takes one tag.
- `--output-json` writes the full results next to the test file, replacing `.yml` with
  `.results.json` (`tests/sales.performance.test.results.json`). With no file argument it
  writes `test-results.json` in the current directory. `--format json` prints the same JSON
  to stdout instead of the pretty report.
- `-v` adds each failing run's actual output to the pretty report. `-q` shows only the
  summary.
- `--min-accuracy` compares **per-file** scores. `--threshold-mode average` (the default)
  checks the mean across files, and `all` checks every file. It doesn't check individual
  cases.
- **Cost:** every case runs `runs` times through the full agent, plus one judge call per run,
  with `concurrency` runs against the warehouse at once. When you iterate, run one case.

## Reading results

The pretty report gives one line per case:

| Status | Meaning |
| --- | --- |
| `PASS (3/3)` | every run passed |
| `FLKY (1/3)` | some runs passed. The answer is unstable, which is a real defect |
| `FAIL (0/3)` | no run passed |

The file's **Correctness** score is the mean over every case × run. Each run scores 1 (PASS)
or 0 (FAIL). There's no partial credit.

In the JSON, each run is a record with `prompt`, `expected`, `actual_output`, `cot` (the
judge's full response), `choice` (`PASS`/`FAIL`) and `score`. The JSON doesn't include the
SQL or the tool calls. Three things to check first:

- **`actual_output` starting with `[ERROR]`** is a run that never produced an answer: a
  clarifying question, `max iterations exceeded`, a connector failure, a timeout. Its error
  text is also in the top-level `errors` array. Errored runs are appended at the **end** of
  `records`, not next to their case, so group records by `prompt`.
- **An answer with no numbers, or rows whose metric values are all empty,** is a failed
  attempt, even if the judge passed it.
- **A `cot` that argues PASS while `choice` is `FAIL`** is usually a judge parse problem, not
  an agent bug. See the judge category in the diagnosis reference.

The full schema, `jq` recipes, and how to see what the agent actually queried:
[references/results-json.md](references/results-json.md).

## Filling expected answers from evidence

1. Run the case, or the whole file if several placeholders need filling, with
   `--output-json`. Re-run if a run failed on a transient error. Up to three runs in total is
   reasonable.
2. For each case, compare `actual_output` across every run and classify it:

| Class | Evidence | What to write |
| --- | --- | --- |
| `stable` | 2+ runs agree on the business facts | a full expected, numbers approximated |
| `stable_but_partial` | ranking or direction agrees, values vary | the ranking plus 2–3 anchors |
| `flaky` | runs disagree materially | only the facts that agree. Flag the rest for repair |
| `ambiguous` | runs answer different readings of the prompt | propose a sharper prompt to the user |
| `cannot_answer_as_asked` | runs consistently decline, with a valid reason | encode the reason and the alternative offered |
| `likely_unsupported` | no run finds the data | leave a narrowed `DRAFT:` note. This is a gap to report |

3. **Check every number you're about to write down.** An observed answer isn't automatically a
   correct one. If you can, verify it independently (for example, run the agent's SQL or a
   simple query yourself). **Never encode a clearly wrong answer as `expected` just because
   it was stable.** A consistently wrong agent is a repair, not a spec.
4. Replace the placeholders, turn relative periods into absolute ones, and report:

```text
## Eval drafting summary
File: tests/sales.performance.test.yml   Cases: 9   Drafted: 6   Partial: 2   Flaky: 1
| Case                 | Class              | Notes                        |
| total-revenue-2025   | stable             | 3/3 runs agree, ~$6.7B       |
| top-items-feb-2026   | stable_but_partial | ranking stable; values vary  |
Follow-up: <cases to repair, prompts to sharpen, data gaps>
```

## Diagnose and repair

When a case fails or is flaky, the agent is the thing under repair. The test stays fixed,
apart from mechanical edits such as adding a missing `name:`.

### 1. Reproduce

```bash
oxy test <file> --case <name> --output-json
```

Read every run of that case: `expected`, `actual_output`, `cot`, `choice`.

### 2. See what the agent did

The results JSON has only the final answer. To see the path, run the same prompt on its own
and read the events:

```bash
oxy agentic run <path/to/agent.agentic.yml> -q "<the case's prompt>" --json
```

This streams the run's events as JSON lines and needs `OXY_DATABASE_URL`. Look at
`query_executed`: its `query`, its `rows`, and its `source`. `source` is `semantic` (compiled
by the semantic model), `llm` (SQL the model wrote), `verified_sql` (a Verified Query), or
`vendor`. Also look at `analytics_validation_failed`, the checks that bounced the agent. If
the project exports traces to Oxy's observability, the run's trace shows the same steps.

### 3. Classify the root cause before editing anything

| Category | Typical evidence | Fix lives in |
| --- | --- | --- |
| A. Semantic model gap: a grouping or dimension is missing | many queries, raw rows, the agent aggregates in prose | a dimension on the view |
| B. Missing or unfindable measure | `source: llm` where a measure should have answered, or arithmetic done in the answer | a measure, or its name and description |
| C. Instructions vague or misdirecting | wrong interpretation, varying definitions across runs | `instructions:` or a `.md` context doc |
| D. Canonical answer exists as SQL | the same question keeps producing different SQL | a Verified Query |
| E. Validation rejects a correct result | `analytics_validation_failed` loops, `max iterations exceeded` | `validation:` |
| F. Judge false negative | `cot` agrees with the expected answer, `choice` is FAIL | report it. Don't touch the agent |
| G. Prompt underspecified | clarifying-question errors, different readings across runs | report it, and propose a sharper prompt |
| H. Data unavailable | no view or table holds it | report it. Don't fake support |

Symptoms, evidence, and a worked fix for each:
[references/diagnosis.md](references/diagnosis.md).

### 4. Repair: levers in an `.agentic.yml` agent, in this order

1. **Semantic model** (`.view.yml` / `.topic.yml` matched by the agent's `context:` globs):
   add the missing measure or dimension, or fix an aggregation or an entity. The agent's
   catalog search matches a measure's **name**, its **description** and its **view name**. It
   doesn't read `synonyms:`, so the words users ask with belong in the description. A view
   that no `context:` glob matches doesn't exist as far as the agent is concerned.
2. **Context** (`context:` globs, relative to the agent file):
   - **`.md` docs** go into the prompts for clarifying the question and writing the answer.
     Use them for business definitions: the fiscal calendar, what "active customer" means,
     which of two similar measures to use.
   - **Verified Queries**: a `.sql` file with a `/* oxy: … */` comment block at the top that
     carries a `description:`. When the agent's search matches its file name or description,
     the SQL runs verbatim, with no SQL generation. A `.sql` file **without** that block is
     only a style example for SQL generation, and is never run as an answer.
3. **Instructions:** top-level `instructions:` reaches every LLM call. `states.<state>.
   instructions:` reaches one stage: `clarifying`, `specifying`, `solving` or `interpreting`.
   Write general rules ("revenue means net of refunds", "rank by units unless asked
   otherwise"). Never put the expected answer, or a number from it, into instructions.
4. **Validation:** the `validation:` block **replaces** the 15 default rules rather than
   adjusting them. To switch off or retune one rule, list every rule that should still run.
   An empty `validation: {}` disables them all.

Make the smallest fix that generalizes. If the same logical gap exists in several views, fix
all of them. Preserve the file's existing style. Explain the root cause alongside the edit.

### 5. Validate

There's no build step. `oxy test` reloads the agent file, its context and its semantic files
from disk on every run, and `oxy build` (which does nothing any more) plays no part. Check
the files you edited, then re-run:

```bash
oxy validate agents/sales.agentic.yml
oxy validate semantics/views/orders.view.yml
oxy test <file> --case <name> --output-json
```

**One pass isn't proof.** A case needs `runs`/`runs` on at least two rounds. Then run the
whole file to catch regressions in other cases. If the repair doesn't hold after three
diagnose-edit-validate cycles, stop and report what you know.

### 6. Report

```text
## Repair summary
Case: tests/sales.performance.test.yml › top-items-feb-2026
Root cause: B — the measure existed, but its description never said "engagements"
Changed: semantics/views/posts.view.yml (description of total_engagements)
Validation: round 1 3/3 · round 2 3/3 · full file 9/9 cases
Notes: <judge observations, remaining risks, anything left for the user>
```

## Never

- Edit `expected`, delete or skip a case, or reword a prompt to make a failure go away.
- Encode an observed answer you haven't checked, or a wrong one because it was consistent.
- Fabricate an expected value. A narrowed `DRAFT:` note is honest, an invented number isn't.
- Hard-code an answer, or a threshold derived from the test data, into instructions or a view.
- Treat a judge false negative as an agent bug, or a clarifying-question error as a wrong
  answer.
- Stop at one lucky pass, or make "while I'm here" edits to unrelated files.
- Explain a root cause you didn't find. "Unclear, needs investigation" is a valid finding.

## Read next

- [references/writing-expected.md](references/writing-expected.md): worked `expected` examples
  by question type, and the evidence rules.
- [references/results-json.md](references/results-json.md): the results schema, `jq`
  recipes, and how the judge's verdict is parsed.
- [references/diagnosis.md](references/diagnosis.md): the failure taxonomy, with a repair
  pattern for each.
- The `oxy-semantic-modeling` skill, for view, measure and entity craft when the repair is in
  the semantic model.
