---
source:
  - sdk/cli/skills/oxy-agents/SKILL.md
  - sdk/cli/skills/oxy-agents/references/quick-reference.md
reconciled-hash: 8949bc70afe8efc72689013e936d508d0854923a
note: |
  Authored condensation of the platform skills listed above. Not auto-synced —
  scripts/sync-skills.sh only copies the verbatim YAML templates. Re-condense
  when a source changes; keep under ~200 lines so the LLM context stays focused.
  scripts/check-skills-drift.sh flags a changed source and prints the hash to record.
---

# Oxy `.agentic.yml` Builder Reference

An Oxy agent is one file, `<name>.agentic.yml`. It configures a **fixed**
pipeline that turns a question into a grounded query against the semantic
model, runs it, checks the result and explains it. You configure the pipeline;
you do not define its states or its tools.

## There is one kind of agent

- **`.agentic.yml` is the agent.** The classic `.agent.yml` format has been
  removed — never create one or copy one from an old workspace.
- **There is no `kind:` field.** Every `.agentic.yml` runs the same analytics
  pipeline; a file named `app_builder.agentic.yml` is not special. Generating
  views or agent files is the Builder Agent (chat Build mode), which no
  `.agentic.yml` configures.
- **Automations are a different file** (`.automation.yml`; `.procedure.yml`
  still works; `.workflow.yml` is no longer recognised). An agent can *run* an
  automation in its context; it cannot *be* one.
- **Instruction-only agents skip the pipeline.** `instructions:` with no
  `databases:`, `context:` or `states:` is one LLM call that queries nothing.

Use an agent for conversational questions over modelled data; a **verified
query** (`.sql` with an `oxy:` header) when one question must always get the
same SQL; an automation when the steps are fixed.

## File

```yaml
instructions: |              # every LLM state's system prompt
  You are a revenue analyst for Acme. Report currency in USD.
timezone: America/New_York   # IANA; falls back to config.yml, then UTC
databases: [warehouse]       # config.yml `databases:` names
context:                     # globs relative to this file's directory
  - ./semantics/**/*.view.yml
  - ./semantics/**/*.topic.yml
  - ./verified/*.sql
  - ./docs/*.md
llm:
  ref: claude                # a `name:` under config.yml `models:`
  extended_thinking: { model: claude-opus-4-6, thinking: adaptive }
states:
  clarifying:   { instructions: "..." }
  interpreting: { thinking: disabled }
```

Minimal file: `llm: { ref: … }` plus `context:` globs for views and topics —
enough when the views declare a `datasource`. Top-level keys: `instructions`,
`timezone`, `databases`, `context`, `llm`, `thinking`, `states`, `validation`,
`semantic_engine` — all optional. **Unknown keys are ignored silently at every
level**, by the runtime and both validators, so a typo (`stats:`, `solve:`)
silently drops the setting. Duplicate keys are a YAML parse error. File rules:
`.agentic.yaml`, a path under a dot-directory / `build/` / `dist/` / `target/`
/ `node_modules/`, or `.test.` in the name is not deployed to the cloud.

## How a question flows

```
clarifying → specifying → solving → executing → interpreting → done
     ↑            ↑           ↑                        ↑
     └──────── diagnosing (when a check fails) ────────┘
```

1. **clarifying** classifies the question (trend, comparison, breakdown,
   single value, distribution, general inquiry, root cause, opportunity),
   searches the catalog and automations, may ask the user. Early exits: a
   general inquiry is answered directly; root cause / opportunity goes to
   metric-tree and anomaly tools; a confident semantic query that compiles, or
   a chosen verified query/automation, skips specifying and solving.
2. **specifying** turns it into a semantic query; if that compiles, **solving
   is skipped**.
3. **solving** writes SQL only when the semantic model could not answer.
4. **executing** runs the SQL (no LLM), then the result checks.
5. **interpreting** writes the answer and picks a chart.

Diagnosing: unresolved metric/join, empty result, compile error or bad SQL →
specifying (fresh attempt); ambiguous column → clarifying; outlier, freshness
gap or bad chart → interpreting, which answers and states the caveat.

**Guidance every answer needs goes in the global `instructions:`,
`clarifying` or `interpreting` — never only in `solving`**, which rarely runs
on a well-modelled workspace.

## Tools by state (fixed; there is no `tools:` key)

| State | Tools |
| --- | --- |
| clarifying | `search_catalog`, `search_automations`, `propose_semantic_query`, `ask_user` |
| root cause / opportunity | `explain_metric`, `find_opportunities`, `metric_sensitivity`, `predict_impact` (+ `list_anomalies`, `detect_anomalies`, `explain_anomaly` with anomaly monitoring) |
| specifying | `search_catalog`, `sample_columns`, `ask_user`; with a semantic model `check_data_freshness`; without, `get_join_path`, `list_tables`, `describe_table` |
| solving | `execute_preview` (candidate query, `LIMIT 5`) |
| executing | none |
| interpreting | `render_chart` |

`search_automations` finds only automations and verified queries this agent's
`context:` reaches. Root cause / opportunity needs a metric tree; metric-tree
and anomaly tools are not offered in workspace previews or scheduled runs.

## Fields

**`instructions`** — persona, business conventions, which views/topics to
prefer. Per-state `instructions` are **appended**, so don't repeat the global
text there.

**`databases`** — the agent queries exactly these plus every database its
context references (a view's `datasource`/`data_source`, a verified query's
`database:`, an automation task's `database:`). **No fallback to every
config.yml database**; with none, the run fails `no databases configured`.

**`context`** buckets by file name:

| Matches | Becomes |
| --- | --- |
| `*.view.yml`, `*.topic.yml` | semantic catalog |
| `*.sql` with an `oxy:` header | **verified query**, run verbatim when chosen |
| `*.sql` without the header | SQL style examples in the solving prompt only |
| `*.md` | domain notes for clarifying and interpreting |
| `*.automation.yml`, `*.procedure.yml` | automations the agent may run |
| anything else | ignored |

```sql
/*
  oxy:
    description: Monthly revenue by region   # used to match the question
    database: warehouse
*/
SELECT ...
```

**`llm`** — `ref` names a config.yml model (vendor, key — a workspace secret
in the cloud — and base URL come from it; prefer it). With neither `ref` nor
`model`, the first config.yml model is used. A `ref` not in config.yml is
**not** a load error: it falls back to a built-in Claude model and the
server's `ANTHROPIC_API_KEY`. `model` alone skips config.yml (key from
`api_key` or server env). `vendor`: `anthropic`, `open_ai` (Responses API),
`open_ai_compat` (`/chat/completions`) — write these spellings; `oxyc validate`
rejects `openai`/`openai_compat`. Google/Gemini is not supported. Leave
`max_tokens` unset (default 4,096, or 16,384 / budget + 4,096 with thinking);
a set value is used as-is and thinking counts against it.

**`states`** honours `clarifying`, `specifying`, `solving`, `interpreting`.
`executing` is accepted and does nothing; any other key (`solve`,
`diagnosing`) is ignored. Each takes `instructions`, `thinking`, `model`
(same provider as `llm:`) and `max_retries` — **tool-call rounds per visit**,
after which the run pauses to ask the user whether to continue; not a
failure-retry count. Defaults: specifying 5, solving 3, interpreting 5;
clarifying's limit is fixed.

**`semantic_engine`** — Cube (`base_url`, `api_token`) or Looker (`base_url`,
`client_id`, `client_secret`) ahead of Oxy's semantic model. `${VAR}` reads
the **server's environment**, not workspace secrets; an unset var or an
unreachable engine at run start fails the run. Omit it normally.

## Thinking

Precedence: extended-thinking toggle → `states.<s>.thinking` → `llm.thinking`
→ top-level `thinking` → state default (**adaptive for solving, disabled for
everything else**). A global value replaces every default, so top-level
`thinking: adaptive` makes every state think — prefer per-state settings.

| Form | Meaning |
| --- | --- |
| `adaptive` | model decides |
| `disabled` | off (bare or quoted; **never** `false`/`true` — parse error) |
| `effort:high` / `{effort: high}` | `low`, `medium`, `high`, `xhigh`, `max`; unknown level → `medium` |
| `{budget_tokens: N}` | hard ceiling, native Claude 3.7–4.6 |
| any other string (`enabled`, `high`) | silently **disabled** |

Anthropic: `adaptive`/`effort` native from 4.6 (`xhigh` from 4.7); 4.7+
rejects `budget_tokens`, so Oxy sends the nearest effort and warns; older
models get a budget, `xhigh` → `high`. OpenAI: `effort` → reasoning effort
(`xhigh`/`max` → `high`), `adaptive` = model default, `budget_tokens` ignored.
`open_ai_compat`: anything but `disabled` adds a chain-of-thought prompt.
Write `effort` on current models. `llm.extended_thinking: {model, thinking}`
replaces both in every state while the user's toggle is on; without it the
toggle does nothing.

## Validation rules

Omit `validation:` and 15 default rules run. **Including it replaces the
defaults**: only listed rules run (`enabled: false` skips one) and a stage
with no list is unchecked — to turn one off, list every rule you still want.
Entry: `- name: <rule>` plus params at the same level (misspelled params are
ignored).

| Stage | Rules (`*` = not default) |
| --- | --- |
| `specified` | `metric_resolves`, `join_key_exists`, `filter_unambiguous` |
| `solvable` | `sql_syntax` (`dialect` accepted, parsing is generic), `tables_exist_in_catalog`, `spec_tables_present`, `column_refs_valid`, `timeseries_order_by_check` |
| `solved` | `non_empty`, `truncation_warning`, `no_nan_inf`, `outlier_detection` (`threshold_sigma` 5.0, `min_rows` 4), `null_ratio_check` (`threshold` 0.5), `duplicate_row_check` (`max_duplicate_ratio` 0.1), `freshness_check` (`threshold_days` 1), `shape_match`*, `timeseries_date_check`* |

## Validate, run, test

- `oxyc validate --file <path>` (offline schema) and `oxy validate --file
  <path>` (runtime parser) — use the builder's `validate_project` tool after
  every edit. Both are **structural only**: they miss unknown keys, `llm.ref`,
  `databases:`, globs, rule names, timezones and `${VAR}`.
- The first full check is a run: `oxy agentic run <path> -q "<question>"`
  (needs Oxy's Postgres; `--json`, `--thinking-mode extended_thinking`).
- `oxy test <file>.test.yml` runs LLM-judged evals (`target:`, `settings:
  {runs, concurrency, judge_model}`, `cases: [{prompt, expected, tags}]`).
- **`oxy build` does not check agents** (it builds search embeddings).
- A JSON schema *is* published for `.agentic.yml`; an editor header on line 1
  is optional — preserve it if present:
  `# yaml-language-server: $schema=https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/agentic.json`.

## Pitfalls

| Symptom | Fix |
| --- | --- |
| `no databases configured` | name a database, or give context views a `datasource` |
| agent can't see a table | add its database to `databases:` |
| wrong model / auth error on first question | `llm.ref` isn't a config.yml `name:` (silent Claude fallback), or the key secret is unset |
| "does not have a metric tree configured" | define a metric tree, or ask for the numbers directly |
| `solving` instructions never apply | move them to global `instructions:` or `clarifying` |
| verified query never chosen | add the `oxy:` header with a `description`; check the glob |
| state override does nothing | misspelled key, `executing`, or `clarifying.max_retries` |
| thinking silently off | unknown string like `enabled` — use a listed form |
| a default check stopped running | a `validation:` section replaced the defaults |
| `validation config error: unknown … rule` | fix the name or move it to its stage |
| `invalid timezone '…'` | use an IANA name (`UTC`, `America/New_York`) |
| `environment variable '${X}' … is not set` | set it in the server environment |
| agent missing in the cloud, fine locally | `.agentic.yaml`, excluded path, or `.test.` in the name |
