---
name: oxy-agents
description: >-
  Use when creating, editing, reviewing or debugging an Oxy agent, a `.agentic.yml` file that
  answers questions from the semantic model by clarifying, specifying, solving (SQL), executing
  and interpreting. Triggers include "create an analytics agent", "my agent picks the wrong
  metric", "my solving instructions are ignored", "the agent never uses my verified query", "the
  agent can't see a database", "which tools can a state call", "set thinking, effort,
  budget_tokens or extended thinking", "per-state model or max_retries", "validation rules",
  "Cube or Looker semantic_engine", "analytics vs app-builder agent", "`.agent.yml` or
  `.agentic.yml`", and validating, running (`oxy agentic run`) or testing (`oxy test`) an agent.
  Not for writing the semantic model itself (views, topics) or automations.
---

# Oxy agents (`.agentic.yml`)

An Oxy agent is one YAML file, `<name>.agentic.yml`. It configures a fixed
pipeline that turns a natural-language question into a grounded query against
the semantic model, runs it, checks the result and explains it. You configure
the pipeline. You do not define its states or its tools.

## There is one kind of agent

- **`.agentic.yml` is the agent.** The classic `.agent.yml` format (a single
  LLM call with custom tools) has been removed. Do not create one, and do not
  copy one from an old workspace.
- **There is no `kind:` field.** Every `.agentic.yml` runs the same analytics
  pipeline. A file named `app_builder.agentic.yml` is not special. Generating
  views or agent files is the in-product **Builder Agent** (chat Build mode),
  and no `.agentic.yml` configures it.
- **Automations are a different file.** Deterministic multi-step work is an
  automation, `.automation.yml` (`.procedure.yml` still works). `.workflow.yml`
  is no longer recognised at all. An agent can *run* an automation it finds in
  its context. It cannot *be* one.
- **Instruction-only agents skip the pipeline.** A file with `instructions:`
  and no `databases:`, `context:` or `states:` is answered with one LLM call
  and queries nothing. Add any of those three and you get the full pipeline.

**Use an agent** for conversational questions over data that has a semantic
model. **Use a verified query** (a `.sql` file with an `oxy:` header, below)
when one question must always get exactly the same SQL. **Use an automation**
when the steps are fixed and need no LLM to choose between them.

## Minimal file

```yaml
llm:
  ref: claude            # a `name:` under config.yml `models:`
context:
  - ./semantics/**/*.view.yml
  - ./semantics/**/*.topic.yml
```

That is enough when the views declare a `datasource`. The agent can query
every database its context references. The full annotated file is
[templates/agentic-template.yml](templates/agentic-template.yml).

Top-level keys: `instructions`, `timezone`, `databases`, `context`, `llm`,
`thinking`, `states`, `validation`, `semantic_engine`. All are optional.
**Unknown keys are ignored silently at every level**, by the runtime and by
both validators. A typo such as `stats:` or `solve:` costs you the setting
with no error.

## How a question flows

```
clarifying → specifying → solving → executing → interpreting → done
     ↑            ↑           ↑                        ↑
     └──────── diagnosing (when a check fails) ────────┘
```

1. **clarifying** classifies the question as a trend, comparison, breakdown,
   single value, distribution, general inquiry, root cause or opportunity. It
   searches the catalog and automations, and may ask the user to clarify.
   Three outcomes leave the normal path early:
   - **General inquiry** ("what metrics do you have?"). Answered directly,
     with no query.
   - **Root cause / opportunity** ("why did revenue drop?", "where can we
     grow?"). Handled by metric-tree and anomaly tools, skipping
     specifying/solving.
   - **Confident semantic query.** If the model proposes the exact measures
     and dimensions with high confidence and the semantic model compiles them,
     specifying and solving are both skipped. Likewise when it selects a
     verified query or automation, that is run instead.
2. **specifying** turns the question into a semantic query. If the semantic
   model compiles it, **solving is skipped** and the compiled SQL runs.
3. **solving** writes SQL itself. This happens only when the semantic model
   could not answer.
4. **executing** runs the SQL. It has no LLM. Then the result checks run.
5. **interpreting** writes the answer and picks a chart.

**Diagnosing** routes a failure back upstream:

- An unresolved metric or join, an empty result, a compile error or bad SQL
  goes back to specifying.
- An ambiguous column goes back to clarifying.
- Outliers, a stale period or a bad chart go to interpreting, which still
  answers and states the caveat.

The practical consequence: **guidance every answer needs goes in the global
`instructions:`, `clarifying` or `interpreting`, never only in `solving`.** On a
well-modelled workspace, solving rarely runs.

## Tools each state can call

The tool set is fixed by the runtime and is not configurable. There is no
`tools:` key.

| State | Tools |
| --- | --- |
| clarifying | `search_catalog`, `search_automations`, `propose_semantic_query`, `ask_user` |
| clarifying, root cause / opportunity | `explain_metric`, `find_opportunities`, `metric_sensitivity`, `predict_impact`, plus `list_anomalies`, `detect_anomalies` and `explain_anomaly` when anomaly monitoring is set up |
| specifying | `search_catalog`, `sample_columns`, `ask_user`. With a semantic model: `check_data_freshness`. Without one: `get_join_path`, `list_tables`, `describe_table` |
| solving | `execute_preview` (runs a candidate query with `LIMIT 5`) |
| executing | none |
| interpreting | `render_chart` |

`search_automations` searches only the automations and verified queries this
agent's `context:` reaches. With none, it finds nothing. The metric-tree and
anomaly tools are offered in interactive runs. They are not offered in
workspace previews or scheduled runs.

## The fields that matter

**`instructions`** go into every LLM state's system prompt. Use them for the
persona, business conventions ("fiscal year starts in February", "customer
means the `customers` view") and which views or topics to prefer. Per-state
`instructions` are **appended** to these, not substituted, so don't repeat the
global text in them.

**`timezone`** is an IANA name used for "today", "yesterday" and "last week".
It falls back to config.yml's `timezone:`, then UTC. An invalid name fails the
run.

**`databases`** lists names from config.yml `databases:`. The agent can query
exactly these plus every database its context files reference: a view's
`datasource` (or `data_source`), a verified query's `database:`, or an
automation task's `database:`. There is **no fallback to every database in
config.yml**. With none at all, the run fails with `no databases configured`.

**`context`** takes globs relative to the agent file's directory. Each match is
bucketed by its file name:

| Matches | Becomes |
| --- | --- |
| `*.view.yml`, `*.topic.yml` | the semantic catalog |
| `*.sql` with an `oxy:` header | a **verified query**: found by `search_automations` and run verbatim |
| `*.sql` without the header | SQL style examples in the solving prompt only |
| `*.md` | domain notes in the clarifying and interpreting prompts |
| `*.automation.yml`, `*.procedure.yml` | automations the agent may pick and run |
| anything else | ignored |

A verified query opts in with a leading comment block:

```sql
/*
  oxy:
    description: Monthly revenue by region
    database: warehouse
*/
SELECT ...
```

Without the header, the same file is only a style hint. Context files under a
dot-directory, `build/`, `dist/`, `target/` or `node_modules/`, or with
`.test.` in the name, are not deployed, so a glob that works on your machine
can come back empty in the cloud.

**`llm`** picks the model.

- `ref` names a model in config.yml `models:`, and the vendor, API key and base
  URL come from that entry. In the cloud the key is a workspace secret, so
  prefer `ref`.
- With neither `ref` nor `model`, the first model in config.yml is used.
- `model` overrides only the model ID. On its own (no `ref`) it uses
  `api_key` or the server's `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`.
- `vendor` (`anthropic`, `open_ai`, `open_ai_compat`) overrides the ref's wire
  protocol. Use `open_ai_compat` for gateways that serve only
  `/chat/completions`.
- Google/Gemini models are not supported in agents.

**`states`** honours four keys: `clarifying`, `specifying`, `solving` and
`interpreting`.

- `executing` has no LLM, so it is accepted and does nothing. Every other key
  is ignored.
- Each state takes `instructions`, `thinking`, `model` and `max_retries`.
- `model` is a provider model ID. It inherits the vendor, key and base URL
  from `llm:`, so it must be the same provider.
- `max_retries` is the number of **tool-call rounds** per visit. When they run
  out, the run pauses and asks the user whether to continue. It is not a
  failure-retry count. Defaults: specifying 5, solving 3, interpreting 5.
  Clarifying has a fixed limit and ignores the setting.

**`validation`** sets the result checks. Leave it out and 15 default checks
run. If you include it, it **replaces** the defaults: only the rules you list
run, and a stage you leave empty is not checked. To switch one check off or
retune it, list every rule that should still run. Rule names and parameters
are in [references/quick-reference.md](references/quick-reference.md).

**`semantic_engine`** routes queries through Cube or Looker ahead of Oxy's own
semantic model. `${VAR}` placeholders are read from the server's environment,
not from workspace secrets. The engine is pinged when a run starts, and an
unreachable engine fails the run.

## Thinking and models

Thinking is resolved per state, in this order:

1. the state's own `thinking`
2. `llm.thinking`
3. top-level `thinking`
4. the state's built-in default: **adaptive for solving, disabled for
   everything else**

A global value replaces every built-in default. So top-level
`thinking: adaptive` makes clarifying, specifying and interpreting think too,
and `llm.thinking: disabled` turns thinking off in solving. Prefer per-state
settings.

| Form | Meaning |
| --- | --- |
| `adaptive` | the model decides when and how much to think |
| `disabled` | no thinking (bare or quoted; **never** `false`, which is a parse error) |
| `effort:high` or `{effort: high}` | depth hint: `low`, `medium`, `high`, `xhigh`, `max` |
| `{budget_tokens: 10000}` | hard token ceiling (Claude 4.6 and earlier) |

How each provider treats these forms:

- **Anthropic.** `adaptive` and `effort` are native from Claude 4.6, and
  `xhigh` from 4.7. Claude 4.7 and later **reject** `budget_tokens`, so Oxy
  sends it as the nearest effort level and logs a warning. On older models it
  goes the other way: `adaptive` and `effort` become a token budget, and
  `xhigh` becomes `high`. Write `effort` on current models.
- **OpenAI** (`open_ai`, Responses API). `effort` maps to reasoning effort,
  with `xhigh` and `max` sent as `high`. `adaptive` leaves the model's default
  effort. `budget_tokens` is ignored.
- **`open_ai_compat`.** Any setting other than `disabled` adds a
  chain-of-thought instruction to the prompt, because there is no native
  control.

A string that isn't one of the forms above (`thinking: enabled`,
`thinking: high`) silently means **disabled**. An unknown effort level
silently means `medium`.

`llm.extended_thinking: {model, thinking}` is used while the user has the
**extended thinking** toggle on (`--thinking-mode extended_thinking` from the
CLI). It replaces the model and the thinking setting in every state. Without
the block, the toggle changes nothing.

Leave `llm.max_tokens` unset unless you need a hard cap. When it is unset, Oxy
uses 4,096 without thinking and 16,384 (or the budget plus 4,096) with
thinking. A value you set is used as-is, and thinking tokens count against it.

## Validate, run, test

```bash
oxyc validate --file agents/sales.agentic.yml   # offline schema check, no server
oxy validate --file agents/sales.agentic.yml    # the runtime's own parser
oxy agentic run agents/sales.agentic.yml -q "Revenue by region last quarter?"
oxy test agents/sales.agentic.test.yml          # LLM-judged evals
```

- **Both validators are structural only.** They catch a bad shape (a
  malformed `thinking`, a wrong type, a duplicate key). `oxyc validate` also
  rejects the vendor spellings `openai` and `openai_compat`, which the runtime
  accepts, so write `open_ai` and `open_ai_compat` to satisfy both. Neither
  validator resolves `databases:` or `llm.ref`, expands globs, reads `${VAR}`
  placeholders, checks rule names or timezones, or notices unknown keys.
- **The first full check is a run.** `oxy agentic run <path> -q "<question>"`
  builds the agent exactly as the server does and streams each state. It runs
  in a workspace directory and needs Oxy's Postgres database (`OXY_DATABASE_URL`),
  like `oxy serve`. Add `--json` for JSONL events, or `--thinking-mode
  extended_thinking`. A CLI run that pauses to ask the user something can't be
  usefully resumed from the CLI, because `oxy agentic answer` only marks the
  run as running and does not deliver the text. Ask a more specific question,
  or continue in the app.
- **`oxy build` does not check agents.** It does nothing any more.
- **`oxy test`** runs `.test.yml` evals against an agent and scores each case
  with an LLM judge. The format and flags are in
  [references/quick-reference.md](references/quick-reference.md).

To point an editor at the schema, start the file with:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/oxy-hq/oxygen/refs/heads/main/json-schemas/agentic.json
```

## Pitfalls

| Symptom | Cause | Fix |
| --- | --- | --- |
| `no databases configured` | nothing in `databases:` and no context file references a database | add a name from config.yml, or views with `datasource` |
| agent can't see a table it "should" | the agent only reaches databases it declares or references | add it to `databases:` |
| wrong model answers, or an auth error on the first question | `llm.ref` is not a `name:` in config.yml, so the agent fell back to a built-in Claude model and the server's `ANTHROPIC_API_KEY`; or the model's key secret is unset | fix the name; set the key |
| "does not have a metric tree configured" | the question was classified as root cause or opportunity, and this workspace or run has no metric tree | define a metric tree, or ask for the numbers directly |
| `solving` instructions never take effect | the semantic model compiled the question, so solving was skipped | move them to the global `instructions:` or `clarifying` |
| verified query never chosen | `.sql` has no `oxy:` header, or isn't matched by `context:` | add the header and a `description`; check the glob |
| state override does nothing | misspelled key, or `executing` / `clarifying.max_retries` | use the four honoured keys |
| thinking silently off | `thinking: enabled` or another unknown string | use one of the forms above |
| `thinking: false` rejected | boolean, not a mode | `thinking: disabled` |
| a default check stopped running | a `validation:` section replaced the defaults | list every rule you still want |
| `validation config error: unknown … rule` | rule name typo, or a rule under the wrong stage | names and stages are in the quick reference |
| `invalid timezone '…'` | not an IANA name | e.g. `America/New_York`, `UTC` |
| `environment variable '${X}' … is not set` | `semantic_engine` placeholder unset on the server | set it in the server environment |
| `semantic engine connection error` | Cube/Looker unreachable or rejected the credentials | check `base_url` and credentials |
| agent missing in the cloud, fine locally | `.agentic.yaml` extension, a dot-directory/`build/` path, or `.test.` in the name | rename or move it |

## Read next

- [references/quick-reference.md](references/quick-reference.md): every field,
  default, rule and parameter in tables, plus the `.test.yml` format.
- [templates/agentic-template.yml](templates/agentic-template.yml): an annotated
  starter file that validates against the schema.
- Related skills: **oxy-semantic-modeling** for the views and topics an agent
  queries, **oxy-evals** for writing and repairing `.test.yml` evals, and
  **oxy-automations** for automations.
