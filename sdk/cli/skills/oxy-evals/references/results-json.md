# Reading eval results

`oxyc evals run <file> --output-json` writes `<file without .yml>.results.json` at the test
file's path, relative to the current directory (`tests/sales.performance.test.yml` →
`tests/sales.performance.test.results.json`), or to the file you name after `--output-json`.
Without a file argument it writes `test-results.json`. `--json` prints the same document on
stdout.

It is the text the deployment's run produced, byte for byte the document the older local
`oxy test --output-json` wrote, so the shape below holds for both.

## Shape

The file is a JSON array with **one element per test file**. Keys appear in the order below
except `test_name`, which is written last; every record also carries `llm_rounds`, and
optional fields (`final_sql`, `answer_text`, `timing`, `runtime`, `model`, …) appear only when
set:

```jsonc
[
  {
    "test_name": "sales.performance.test.yml",
    "errors": [                       // one string per run that produced no answer
      "agentic pipeline asked a clarifying question: Which region do you mean?"
    ],
    "stats": {
      "total_attempted": 9,           // cases × runs
      "answered": 8                   // runs that returned an answer (judged or not)
    },
    "metrics": [
      {
        "type": "Correctness",        // the only metric an eval produces
        "score": 0.6667,              // mean of every record's score in this file
        "records": [
          {
            "prompt": "What was total revenue across all stores in 2025?",
            "expected": "Total revenue across all stores in 2025 was approximately $6.7 billion.",
            "actual_output": "Total revenue in 2025 was $6.74B across 45 stores…",
            "cot": "1. Key facts in the expected answer… Verdict: PASS",
            "choice": "PASS",         // PASS or FAIL
            "score": 1.0,             // 1.0 for PASS, 0.0 for FAIL — never partial
            "duration_ms": 18234.5,
            "input_tokens": 41210,
            "output_tokens": 1830
          }
          // … one record per run: `runs: 3` gives 3 records per case
        ]
      }
    ]
  }
]
```

What is **not** in it: the SQL, the tool calls' inputs and outputs, the rows the agent saw. Records for an
`.agentic.yml` target carry no `references` array. Diagnose the path with the commands under
"Seeing what the agent did" below.

## Field by field

| Field | Use it for |
| --- | --- |
| `prompt` | Grouping. A case's runs share the same prompt string. |
| `expected` | The specification, exactly as written in the test file. |
| `actual_output` | The agent's final answer. This is your main evidence. `[ERROR] …` means the run failed. |
| `cot` | The judge's whole response: its reasoning, then its verdict line. |
| `choice` | The parsed verdict. Compare it with `cot` to spot judge parse problems. |
| `score` | `choice` as a number. |
| `duration_ms`, `*_tokens` | Cost and latency of the agent run, not of the judge. Zero on errored runs. |
| `observed` | What the run did, read off its events: `tools` (every call's name, in order), `query_sources` (each successful query's source), `answer_source`, `preagg`, `charts`, and `asked_user` (the clarifying question it stopped on). Absent on errored runs. |
| `unmet_expectations` | The case's `expect:` checks the run failed, each named. Absent when all were met or none were set. |
| `errors` | The error text of every run that never answered, in run order. A clarifying question its case expects (`asks_user: true`) is not an error; any other is. |
| `stats.answered` vs `total_attempted` | The answer rate. A gap means runs ended in errors. |

## Ordering

Answered runs come first, in case order. **Errored runs are appended at the end of
`records`**, each with `choice: "FAIL"`, `actual_output: "[ERROR] <message>"` and
`cot: "Run failed with error: <message>"`. Don't assume a case's runs are adjacent. Group by
`prompt`.

## How the verdict is parsed

The judge gets one prompt: the question, the expected answer and the agent's answer. It's
asked to list the expected answer's key facts, check whether the agent's answer contains them,
look for contradictions or significant omissions, ignore formatting, style, phrasing and level
of detail, allow extra correct information, and end with `PASS` or `FAIL`.

The verdict is read from the **last five lines** of the judge's response:

- A line that contains `PASS` and not `FAIL` → PASS.
- A line that contains `FAIL` → FAIL. That includes a line containing both words, such as an
  echoed `Verdict: PASS or FAIL`.
- No verdict in the last five lines → **FAIL**.

So a judge false negative usually looks like this: the `cot` reasoning concludes the answer
matches, but the verdict is missing, buried higher up, or written as "PASS or FAIL". The fix
for that is the judge model, never the agent.

## Recipes

Without tools: for a single `--case` run, the file is small, so read it whole. For a full
suite, search the file for a distinctive phrase from the prompt and read the records around
each hit.

With `jq`:

```bash
R=tests/sales.performance.test.results.json

# Pass count per case
jq -r '.[].metrics[] | select(.type=="Correctness") | .records
       | group_by(.prompt)[] | "\(map(.score)|add|floor)/\(length)  \(.[0].prompt)"' "$R"

# Every answer to one case, side by side
jq -r '.[].metrics[].records[] | select(.prompt | test("total revenue"; "i"))
       | "\(.choice)  \(.actual_output)\n"' "$R"

# Failing runs with the judge's reasoning
jq -r '.[].metrics[].records[] | select(.score < 1)
       | "## \(.prompt)\n\(.actual_output)\n--- judge ---\n\(.cot)\n"' "$R"

# Runs that never answered
jq -r '.[].errors[]' "$R"
```

## Seeing what the agent did

The results only show answers. To see the route an answer took, replay the prompt:

```bash
oxyc agentic run <path/to/agent.agentic.yml> -q "<prompt>" --json --workspace <id>
```

It runs the agent the deployment has compiled (push first), as the evals do, and needs only an
`oxyc` login. It prints one JSON object per line, `{"seq", "event_type", "payload"}`; each
`payload` also carries the run's `attempt`. The events that matter for
diagnosis:

| `event_type` | What to read in `payload` |
| --- | --- |
| `semantic_shortcut_attempted` / `semantic_shortcut_resolved` | whether the semantic model answered directly |
| `spec_resolved` | the tables, joins and assumptions the agent settled on |
| `query_generated` | SQL the model wrote |
| `query_executed` | `query`, `row_count` (the full count), `rows` (a preview: a react run sends the first 10), `error`, and `source`: `semantic`, `llm`, `verified_sql` or `vendor` |
| `result_table` | a react run only: the answer's result, its first 50 `rows`, `columns`, `column_kinds`, and `row_count` |
| `result_chart` | a react run only: a chart the agent drew, `kind`, `x`, `y`, `series`, and its own `rows` as `[x, series?, y…]` |
| `analytics_validation_failed` | `state`, `reason`: a check that sent the agent back to retry |

Run it two or three times for a flaky case. Different `source` values or different SQL across
runs is the flakiness, made visible.

## The summary

Without `--json`, `oxyc evals run` prints one line per test file (its name, the Correctness
score and `answered x/y`), then, indented, `FAIL` and the prompt of every run that didn't
pass. The judge's reasoning, the actual answers and the errors are only in the results
document.
