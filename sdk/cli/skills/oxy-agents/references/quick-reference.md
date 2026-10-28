# `.agentic.yml` quick reference

Dense tables. The reasoning behind them is in [../SKILL.md](../SKILL.md); a
starter file is [../templates/agentic-template.yml](../templates/agentic-template.yml).

## File

| Rule | Detail |
| --- | --- |
| Name | `<name>.agentic.yml`. `.agentic.yaml` is not deployed to the cloud |
| Location | anywhere in the workspace, except under a dot-directory, `build/`, `dist/`, `target/` or `node_modules/` |
| Not deployed | any file with `.test.` in its name (that is the eval-file convention) |
| Referenced as | its workspace-relative path, e.g. `agents/sales.agentic.yml`. The UI shows the stem (`sales`) |
| Unknown keys | ignored silently at every level, by the runtime and by both validators |
| Duplicate keys | a YAML parse error |
| Editor schema | `# yaml-language-server: $schema=https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/agentic.json` |

## Top-level fields

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `instructions` | string | none | Added to every LLM state's system prompt |
| `timezone` | IANA name | config.yml `timezone:`, then UTC | Used for "today" and relative dates. An invalid name fails the run |
| `databases` | list of config.yml database names | `[]` | Combined with the databases the context references. There is no all-databases fallback |
| `context` | list of globs | `[]` | Relative to the agent file's directory |
| `llm` | map | see below | Model, key, thinking |
| `thinking` | thinking form | none | Global thinking. Overrides each state's built-in default; `llm.thinking` overrides it |
| `states` | map | `{}` | Per-state overrides |
| `validation` | map | 15 default rules | **Replaces** the defaults when present |
| `semantic_engine` | map | none | Cube or Looker ahead of Oxy's semantic model |

An instruction-only agent (`instructions`, and no `databases`, `context` or
`states`) is one LLM call. It runs no pipeline and no queries.

## `llm:`

| Field | Default | Notes |
| --- | --- | --- |
| `ref` | none | A `name:` under config.yml `models:`. Supplies the vendor, key (a workspace secret in the cloud), base URL, Azure settings and headers |
| `model` | from `ref`, else the first config.yml model | A provider model ID. Setting `model` without `ref` skips config.yml entirely, so the key must come from `api_key` or the server environment |
| `vendor` | from `ref`, else `anthropic` | `anthropic`, `open_ai` (Responses API) or `open_ai_compat` (`/chat/completions`). The runtime also accepts `openai` and `openai_compat`; `oxyc validate` does not |
| `api_key` | from `ref`, else `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` on the server | Prefer `ref` |
| `base_url` | from `ref`, else the provider default | `open_ai_compat` defaults to `http://localhost:11434/v1` |
| `max_tokens` | 4,096 without thinking; 16,384 (or budget + 4,096) with thinking | A value you set is used as-is, and thinking tokens count against it |
| `thinking` | none | Beats top-level `thinking` |
| `extended_thinking` | none | `{model, thinking}`. Replaces both in every state while the user's extended-thinking toggle is on |

A `ref` that is not in config.yml is not an error at load time. The agent falls
back to a built-in Claude model and the server's `ANTHROPIC_API_KEY`.
Google/Gemini models are not supported in agents.

## `states:`

| Key | LLM | Thinking default | `max_retries` default | Prompt extras |
| --- | --- | --- | --- | --- |
| `clarifying` | yes | disabled | fixed; the setting is ignored | `.md` context docs |
| `specifying` | yes | disabled | 5 | SQL dialect |
| `solving` | yes | adaptive | 3 | SQL dialect, `.sql` examples |
| `executing` | **no** | n/a | n/a | key accepted, does nothing |
| `interpreting` | yes | disabled | 5 | `.md` context docs |

Any other key (`solve`, `diagnosing`, …) is ignored silently.

| Per-state field | Notes |
| --- | --- |
| `instructions` | **Appended** after the global `instructions` for this state |
| `thinking` | Any thinking form. Highest precedence (except the extended-thinking toggle) |
| `model` | Provider model ID. Inherits the vendor, key and base URL from `llm:`, so it must be the same provider |
| `max_retries` | Tool-call rounds per visit. When they run out, the run pauses and asks the user whether to continue |

## Thinking

| Form | Meaning |
| --- | --- |
| `adaptive` | the model decides |
| `disabled` / `"disabled"` | off |
| `effort:<level>` / `{effort: <level>}` | `low`, `medium`, `high`, `xhigh`, `max`. An unknown level means `medium` |
| `{budget_tokens: N}` | hard ceiling. Native on Claude 3.7 to 4.6 |
| any other string | silently **disabled** |
| `false` / `true` | parse error |

Precedence, highest first: the extended-thinking toggle, then `states.<s>.thinking`,
then `llm.thinking`, then top-level `thinking`, then the state default.

| Provider | `adaptive` | `effort` | `budget_tokens` |
| --- | --- | --- | --- |
| Anthropic, Claude 4.7+ | native | native, all 5 levels | API rejects it; Oxy sends the nearest effort level and warns |
| Anthropic, Claude 4.6 | native | native; `xhigh` → `high` | native |
| Anthropic, 3.7 to 4.5 | sent as a mid-range budget | sent as a budget; `xhigh` → `high` | native |
| OpenAI (`open_ai`) | the model's default reasoning | reasoning effort; `xhigh` and `max` → `high` | ignored |
| `open_ai_compat` | chain-of-thought prompt | chain-of-thought prompt | chain-of-thought prompt |

## `context:` buckets

| Matches | Becomes | Also |
| --- | --- | --- |
| `*.view.yml`, `*.view.yaml` | semantic catalog | its `datasource` / `data_source` adds a database |
| `*.topic.yml`, `*.topic.yaml` | semantic catalog | |
| `*.sql` with an `oxy:` header | verified query, run verbatim when chosen | header `database:` adds a database |
| `*.sql` without the header | SQL examples in the solving prompt | |
| `*.md` | domain notes for clarifying and interpreting | |
| `*.automation.yml/.yaml`, `*.procedure.yml/.yaml` | automations `search_automations` can return | task `database:` values add databases |
| anything else | ignored | |

Verified-query header fields: `description` (used to match the question) and
`database` (the connection to run on).

```sql
/*
  oxy:
    description: Weekly sales for all stores
    database: warehouse
*/
```

## Tools by state (fixed; no `tools:` key)

| State | Tools |
| --- | --- |
| clarifying | `search_catalog`, `search_automations`, `propose_semantic_query`, `ask_user` |
| clarifying → root cause / opportunity | `explain_metric`, `find_opportunities`, `metric_sensitivity`, `predict_impact`, `search_catalog`, `search_automations`, `propose_semantic_query`; with anomaly monitoring also `list_anomalies`, `detect_anomalies`, `explain_anomaly` |
| specifying | `search_catalog`, `sample_columns`, `ask_user`; with a semantic model `check_data_freshness`; without one `get_join_path`, `list_tables`, `describe_table` |
| solving | `execute_preview` |
| executing | none |
| interpreting | `render_chart` |

Root-cause and opportunity questions need a metric tree. Without one the agent
replies that the workspace "does not have a metric tree configured". Metric-tree
and anomaly tools are not offered in workspace previews or scheduled runs.

## `validation:`

Absent: the 15 rules marked default run. Present: only the listed rules run (an
entry with `enabled: false` is skipped), and a stage with no list is not
checked. Entry shape: `- name: <rule>`, optional `enabled`, plus the rule's
parameters at the same level. Misspelled parameters are ignored silently.

| Stage (checked after) | Rule | Default | Params (default) | Fails when |
| --- | --- | --- | --- | --- |
| `specified` (specifying) | `metric_resolves` | yes | | a resolved metric's table or column is not in the catalog |
| | `join_key_exists` | yes | | a join's tables or key are not in the catalog |
| | `filter_unambiguous` | yes | | a filter column is unknown or matches several tables |
| `solvable` (solving) | `sql_syntax` | yes | `dialect` (`generic`): accepted, but parsing is generic whatever its value | the SQL does not parse |
| | `tables_exist_in_catalog` | yes | | `FROM`/`JOIN` names a table the catalog lacks |
| | `spec_tables_present` | yes | | a table the spec resolved is missing from the SQL |
| | `column_refs_valid` | yes | | a `table.column` reference does not exist |
| | `timeseries_order_by_check` | yes | | a time series has no outer `ORDER BY` |
| `solved` (executing) | `non_empty` | yes | | zero rows |
| | `truncation_warning` | yes | | a scalar or series result hit the row cap |
| | `no_nan_inf` | yes | | NaN or ±∞ in a numeric cell |
| | `outlier_detection` | yes | `threshold_sigma` (5.0), `min_rows` (4) | a value is beyond the z-score threshold |
| | `null_ratio_check` | yes | `threshold` (0.5) | a numeric column's NULL share is above the threshold |
| | `duplicate_row_check` | yes | `max_duplicate_ratio` (0.1) | the duplicate-row share is above the ratio |
| | `freshness_check` | yes | `threshold_days` (1) | the requested range runs past the latest date in the result |
| | `shape_match` | no | | the result doesn't have the expected shape |
| | `timeseries_date_check` | no | | a time series' first column is not date-like |

Where a failure goes (diagnosing):

| Failure | Back to |
| --- | --- |
| unresolved metric or join, empty result, semantic compile error | specifying |
| SQL syntax, null ratio or duplicates found after solving | specifying (fresh attempt) |
| ambiguous column | clarifying |
| outlier, freshness gap, bad chart config | interpreting (answers and states the caveat) |

## `semantic_engine:`

| Field | Cube | Looker |
| --- | --- | --- |
| `vendor` | `cube` | `looker` |
| `base_url` | required | required |
| `api_token` | used | n/a |
| `client_id`, `client_secret` | n/a | used |

`${VAR}` is read from the **server's environment**. If the variable is unset,
or the engine is unreachable at run start, the run fails.

## Commands

| Command | Checks |
| --- | --- |
| `oxyc validate --file <path>` | JSON-schema shape, offline. Catches wrong types, a malformed `thinking` and vendor spelling. Misses unknown keys, `ref`, databases, globs, rule names, env vars |
| `oxy validate --file <path>` | parses with the runtime's own parser. Structural only, with the same blind spots |
| `oxy agentic run <path> -q "<question>"` | the real build and run, streamed by state. `--json`, `--thinking-mode extended_thinking`. Needs Oxy's Postgres (`OXY_DATABASE_URL`) |
| `oxy test [<file>.test.yml]` | LLM-judged evals; with no file, every `*.test.yml` in the workspace |
| `oxy build` | not an agent check: it does nothing any more |

## `.test.yml` (evals)

```yaml
target: agents/sales.agentic.yml   # workspace-relative; omit to infer it
settings:
  runs: 3                          # runs per case (default 3), used for consistency
  concurrency: 5                   # default 5
  judge_model: gpt-5-mini          # config.yml model name; use a different family
cases:
  - name: weekly-total             # optional
    prompt: "What were total sales last week?"
    expected: "Total sales were $1.2M"
    tags: [sales, critical]
```

| Detail | Value |
| --- | --- |
| Target inference | `sales.agentic.test.yml` tests `sales.agentic.yml` in the same directory |
| Target kind | must be an `.agentic.yml` |
| `tool:` on a case | parsed, not yet checked |
| Useful flags | `--tag <t>`, `--case <index-or-name>`, `--min-accuracy 0.8` (exit 1 below), `--threshold-mode average\|all`, `--format json`, `--output-json`, `-v` |

## Errors → fix

| Message | Fix |
| --- | --- |
| `no databases configured` | name a database in `databases:`, or give context views a `datasource` |
| `YAML parse error: …` | malformed YAML, a duplicate key, `thinking: false`, or an unknown `semantic_engine.vendor` |
| `validation config error: unknown <stage> rule: '…'` | fix the rule name or move it to its stage |
| `validation config error: invalid params for rule '…'` | fix the parameter's type |
| `invalid timezone '…'` | use an IANA name |
| `environment variable '${X}' referenced in config is not set` | set it in the server environment |
| `semantic engine connection error: …` | check `base_url` and credentials |
| `glob pattern error: …` | fix the `context:` pattern |
