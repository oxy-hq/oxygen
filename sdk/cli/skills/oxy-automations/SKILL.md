---
name: oxy-automations
description: >-
  Use when writing, debugging, or reviewing an Oxy automation (a `.automation.yml` / legacy
  `.procedure.yml` workflow — tasks, loops, variables, schedules) in a customer workspace, or when
  an automation *run* misbehaves: an `http_request` task that works locally and fails in cloud, a
  file that is silently not an automation, a task that validates but fails with "unknown task
  type", SQL that receives JSON instead of a table, or an automation run landing
  `completed_with_errors`. Covers the file-naming trap, task-type gotchas, running SQL and
  automations from the `oxy` CLI, and where the authoritative reference lives.
---

# Oxy automations

**The reference is upstream and maintained — read it rather than guessing:**
<https://www.oxygen-hq.com/docs/guide/build/automations>
(`index`, `loops`, `task-types`, `variables`).

This skill deliberately does **not** restate the schema. It carries the things
that reference does not tell you, and the mistakes that cost time.

## The file-naming trap

The canonical extension is **`.automation.yml`**.

- `.procedure.yml` is the **legacy** name. It is still accepted, so a workspace
  full of it keeps working and nothing warns you — which is exactly why it
  spreads. One production workspace has 18 automations, and every one of them
  is still `.procedure.yml`.
- `.workflow.yml` is **no longer a recognised file kind at all**. A file with
  that name is silently not an automation, and `oxy run` refuses it.
- `oxy migrate-automations` renames legacy files (and rewrites references to
  them); `--dry-run` previews. It is the only thing that reads `.workflow.yml`
  files for that purpose.

**Write new files as `.automation.yml`.** Do not copy the extension from a
neighbouring file in the workspace you are editing — that is how the legacy name
propagated in the first place.

## Naming, and why the code disagrees with the docs

"Automation" is the current name for what used to be called a Procedure, and
before that a Workflow. The sub-automation task is still `type: workflow`, and
API payloads and stored run records still say `workflow` — those are **wire and
storage contracts**, not stale naming to be tidied up. Seeing `workflow` in a
payload does not mean you are looking at something deprecated.

Routes follow the same split: `/automations/:id` is canonical;
`/workflows/:id`, `/procedures`, and `/agentic-workflows` are aliases.

## Task-type gotchas

- **A misspelled `type:` passes validation and fails only at run time.** An
  unrecognised task type — `type: sql`, `type: execute` — is read as an
  "unknown" step rather than rejected, so `oxy validate` reports the file valid
  and the run fails on that step with `unknown task type`. Check every `type:`
  against the task-types page by eye, then *run* the automation; validation
  alone proves nothing here. (A wrong *field* inside a known type, such as
  `query:` instead of `sql_query:`, does fail validation.)
- **`{{ step_name }}` renders as JSON rows, not as a table.** A prior SQL
  step's result renders into a template as a compact JSON list of row objects
  (`[{"region": "west", "total": 42}, …]`). That is what an `agent` or
  `formatter` prompt wants, and what a loop iterates over — but `SELECT … FROM
  {{ extract }}` sends that JSON to the warehouse as SQL and fails. To chain SQL
  steps, either inline the earlier query as a CTE in the later step, or have
  the earlier step write a table (`CREATE OR REPLACE TABLE …`) and read the
  table by name.
- **`http_request` is HTTPS-only and blocks private egress.** Requests to
  localhost, cloud-metadata endpoints, and private IP ranges are refused unless
  that task opts in with `allow_hosts`. A task that works locally and fails in
  cloud is usually this, not a network problem.
- **Partial failures are their own state.** A run where some steps failed records
  as `completed_with_errors`, not as a failed run. Code and dashboards that only
  check for "failed" will report it as a success.

## Running locally with `oxy run`

`oxy run <file>` takes a `.sql` file, an `.automation.yml`, or a
`.procedure.yml` — nothing else.

- **SQL files:** `oxy run report.sql --database warehouse -v start=2024-01-01
  -v region=west`. `--database` picks a database from `config.yml` (default:
  the configured default database). `-v` / `--variables` fills Jinja
  placeholders, and those `-v` values are the **entire** template context —
  nothing else (no database or schema objects) is available inside the SQL.
  `--dry-run` previews the rendered SQL without executing it.
- **No `-v` means no rendering at all.** Run a templated `.sql` file without a
  single `-v` and its `{{ … }}` placeholders reach the warehouse literally, as
  a syntax error. Pass at least one variable whenever the file has
  placeholders.
- **`--dry-run` and `-v` are ignored for automations.** Both apply to `.sql`
  files only. `oxy run x.automation.yml --dry-run` executes the automation for
  real — every write, every `http_request` — and uses the file's own
  `variables:` defaults whatever `-v` says. To check an automation without side
  effects, run its SQL files individually with `--dry-run`.
- `--retry` and `--retry-from <step>` are accepted but currently have no effect
  on an automation run; a failed run is re-run from the top.

## Before you write one

Check whether the work belongs in an automation at all. Scheduled data movement
is usually an Airway pipeline (`.airway.yml`); a question answered from the
semantic model is usually an agent. Automations are for
multi-step orchestration you want to run on a schedule or trigger.

**But "usually a pipeline" is not "never an automation".** A `type: airway`
task runs a pipeline as a step, and the step completes only once the
pipeline's load has committed — the task-types page documents the task, but not
why that guarantee matters. It is what makes the standard
ingest-then-aggregate automation safe: `type: airway` followed by
`type: execute_sql` building a rollup table. Reach for it whenever a dashboard
would otherwise aggregate at query time. **`oxy-data-architecture`** owns that
pattern and the guarantee behind it.
