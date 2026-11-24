---
name: oxy-automations
description: >-
  Use when writing, debugging, or reviewing an Oxy automation (a `.automation.yml` / legacy
  `.procedure.yml` workflow — tasks, loops, variables, schedules) in a customer workspace, or when
  an automation *run* misbehaves: an `http_request` task that works locally and fails in cloud, a
  file that is silently not an automation, a task that validates but fails with "unknown task
  type", SQL that receives JSON instead of a table, or an automation run landing
  `completed_with_errors`. Covers the file-naming trap, task-type gotchas, running SQL and
  automations on a deployment with `oxyc run`, and where the authoritative reference lives.
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
  that name is silently not an automation, and `oxyc run` refuses it.
- `oxyc migrate-automations [path]` renames legacy files (and rewrites
  references to them in every `.yml` / `.yaml` / `.sql` file); `--dry-run`
  previews. It is local only, needs no login, and is the only thing that reads
  `.workflow.yml` files for that purpose. It never overwrites: a file whose
  `.automation.yml` target already exists, or a `.procedure.yml` beside a
  `.workflow.yml` of the same name, is reported and skipped (still exit 0), so
  read its warnings and resolve those by hand. The Rust
  `oxy migrate-automations` only prints this command now.

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

## Running one with `oxyc run`

`oxyc run <file> --workspace <id>` (`npm i -g @oxy-hq/cli`) takes a `.sql` file,
an `.automation.yml`, or a `.procedure.yml` — nothing else. It runs **on the
deployment**, never on this machine. The Rust `oxy run` is gone: it now prints
the `oxyc` command and exits `2`.

- **An automation runs what is deployed, not your edit.** The server runs the
  definition it has compiled for the workspace's served revision, on the worker
  fleet. Push the change first, or try a branch with
  `oxyc preview run <branch> procedure <ref> --workspace <id>` (its writes are
  held). An automation the deployment does not have exits `5` before anything
  starts.
- **`-v name=value` sets the run's variables**, overriding the file's own
  `variables:` defaults. Every value arrives as a string.
- **SQL files:** `oxyc run report.sql --database warehouse -v start=2024-01-01
  region=west --workspace <id>`. The file's text is sent, and runs against the
  deployed database of that name with the workspace's secrets and your
  identity. `--database` is required when the workspace has more than one
  database (`config.yml`'s `defaults.database` is not visible to `oxyc`).
- **`-v` renders only `{{ name }}`, and only when given.** Any other Jinja
  (`{% … %}`, filters, expressions) or a placeholder with no value exits `2`
  and sends nothing. Run a templated file without a single `-v` and its
  `{{ … }}` reach the warehouse literally, as a syntax error.
- **There is no dry run.** `--dry-run`, `--retry` and `--retry-from` are gone.
  A run executes for real — every write, every `http_request`. To try an
  automation without side effects, use the branch preview above.
- A result stops at 10,000 rows. `--json` prints the rows (header row first)
  or the run's final snapshot. Exit `1` the run failed, was cancelled or timed
  out, or `--timeout` ran out (the run carries on: re-running starts a second
  one); `6` the query failed; `7` retryable, before anything started.

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
