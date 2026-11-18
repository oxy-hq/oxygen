---
name: oxy-airway-pipelines
description: >-
  Use when writing, reviewing, or debugging an Oxy Airway ELT pipeline (`.airway.yml`) — sources,
  the `kind` values Oxy actually wires, credentials, reset, rewind and retry — or when one
  misbehaves: "unsupported source kind", a pipeline that parses then fails at run time,
  credentials that do not resolve, a *pipeline* run landing `completed_with_errors` (an
  *automation* run with that status is `oxy-automations`), a schema that will not change, missing
  history after a backfill, or duplicate rows. Carries the traps the published pipeline reference
  does not.
---

# Oxy Airway pipelines

**The reference is upstream — read it first:**
<https://www.oxygen-hq.com/docs/guide/build/data-infrastructure/airway/pipelines>
(keys, supported `kind` values, credentials, how a run works), with the
overview beside it at
<https://www.oxygen-hq.com/docs/guide/build/data-infrastructure/airway>.

That page owns the key table and the basic run model. This skill does not
restate them; it carries the traps that cost time, from one production
workspace's 19 pipelines.

## A pinned engine, so the wired set moves

Airway is an external engine. Each Oxy release pins one engine version and
gives a concrete connector to a **subset** of the sources that engine ships.
The lists below were checked against the engine Oxy pinned in October 2026
(airway 0.1.51). A newer Oxy may wire more; when this page and a run disagree,
the run's `unsupported source kind` error is the authority.

## Anatomy traps

- **Strict at the top, strict-later inside `config:`.** An unknown top-level
  key is a parse error. The `config:` block is free-form when the file is
  parsed and checked only when a run builds the connector — and most kinds
  then reject an unknown key. So a typo inside `config:` sails through
  compilation and fails the first run with `invalid <kind> config: unknown
  field`.
- **Use the reference destination form** — `destination: { database,
  dataset_name }`, resolved to a `config.yml` database at run time. An inline
  destination shape also parses, but it exists for tests; no real pipeline uses
  it.
- **`name` must not start with `preview:`** — the prefix is reserved and
  refused.
- **Off the beaten path:** across 19 real files, `resources`,
  `allow_concurrent_runs`, `streaming` and `channel_capacity` appear **zero**
  times. They work, but there is no house precedent to copy.

## Source kinds: what is wired

**Wired (17):** `rest_api`, `http_file`, `filesystem`, `google_sheets`,
`sql_database`, `postgres_cdc`, `clickhouse`, `toast`, `ubereats`, `sp_api`,
`amazon_ads`, `quickbooks`, `netsuite`, `weather`, `besttime`, `overture`,
`overpass`.

**Shipped by the engine but not wired — unavailable in Oxy:** `shopify`,
`stripe`, `github`, `hubspot`, `salesforce`, `slack`, `jira`, `notion`,
`zendesk`, `google_ads`, `google_analytics`, `facebook_ads`, `airtable`,
`mongodb`, `kafka`, `kinesis`, and more. `kind` is a free-text field, so
`kind: shopify` parses perfectly and fails only when the pipeline runs.
**Reading the engine's own source list and concluding a `kind` works here is
the most expensive mistake this file format offers.**

**`rest_api` is the escape hatch.** The engine's vendor connectors (`shopify`,
`github`, `stripe`, …) are built on its generic REST source, so most can be
written directly as a `rest_api` config with its own pagination and `auth`
settings. A source with no dedicated kind is a `rest_api` config, not a blocker.

**No Excel.** `filesystem` reads `json`, `jsonl` and `csv` only (from a local
path or `s3://`, `gs://`, `az://` plus a glob `pattern`); `ubereats` reads its
`.csv` reports only; `http_file` downloads a file over HTTP. An `.xlsx`
workbook has **no Airway path** — read as CSV it is binary noise. Export it to
CSV, or keep the data in a Google Sheet and use `google_sheets`.

**Destinations** are `airhouse` (production — where every real pipeline lands)
and `postgres`; a `memory` destination exists for tests only.

### `quickbooks`: two custody modes, never mixed

QuickBooks is the one source whose token the host must plumb in, and it comes
in two mutually exclusive custody modes. A config that mixes them **fails
closed** rather than guessing:

- **Rotating custody** — `client_secret_var` + `refresh_token_var`: the
  pipeline refreshes against Intuit itself and stores the new refresh token.
- **Read-only custody** — `access_token_var` alone: some *other* component owns
  rotation, and the value is re-read per request rather than substituted once,
  since a 60-minute token pinned at the start of a long backfill expires
  mid-run.

**Why that matters far beyond this file:** Intuit voids the old refresh token
whenever it issues a new one, so two components rotating the same grant
deadlock into `invalid_grant` — each invalidating the other's — until a human
re-authorises by hand. Exactly one component may ever hold rotating custody;
`access_token_var` is how you keep a pipeline out of that role.

Hence QuickBooks pipelines in one workspace are often **not one pattern**: a
base pipeline that rotates, and per-location pipelines that are read-only
because a scheduled job owns rotation. Copying "the QuickBooks pipeline" as a
template means first knowing which custody mode that company's secret was
provisioned under.

### `google_sheets`: store the key, never a token

- Set **`service_account_json_var`** to a secret holding the service-account
  **JSON key**. A fresh, read-only access token is minted from it on every run.
  Do not store an access token: it lives an hour, so the pipeline would succeed
  once and then return 401 forever. With no key resolved, the run fails with
  `no access token was resolved` rather than a misleading "not found".
- **`ranges:` decides the resource names.** Each entry (`Main`, `Main!A:S`)
  becomes one resource named after the part before `!`, lowercased, spaces
  turned into underscores — `Main!A:S` is the resource `main`, and that is the
  name `resources:` has to list. Empty `ranges` loads every sheet as one
  resource called `sheet_data`.

### `sp_api` (Amazon): two settings with no safe default

- **`default_start` is the entire backfill policy.** The connector only pulls
  *forward*, from one window spanning `default_start` to now, with no
  chunking. Too recent loses history *quietly* — nothing signals it, and only a
  cursor rewind brings it back. Too far back is worse: a window too large for
  Amazon to build in time fails identically on every subsequent run, so the
  pipeline stalls permanently instead of catching up. Pick it deliberately.
- **`partner_type` (`seller`, the default, or `vendor`) belongs to the
  credential.** It decides which reports exist at all; the wrong value pulls
  nothing and refuses every named resource. A business with both accounts
  needs two pipelines.

### `amazon_ads` (Amazon Ads): reports only, and slow

- **Resources:** `sp_search_term` (Sponsored Products search terms) and
  `sp_placement` (campaigns by placement: `top_of_search`, `rest_of_search`,
  `product_pages`, `off_amazon`), both daily. The source only requests and
  downloads reports; it never changes a bid, budget or campaign.
- **Required:** `client_id`, `profile_id` (the advertiser profile, digits
  only; a number or a string), and `default_start` (`YYYY-MM-DD`). A config
  without `default_start` is refused. Amazon keeps about 90 days of these
  reports, so an earlier date is clamped to that, and a later one loses
  history silently.
- **`region` is `NA` only** today; another value is refused.
- **A report takes 10–50 minutes to build.** `poll_budget_minutes` (at most
  240) bounds how long one run waits. A report still pending when the run
  ends is picked up by the next run, but only on a destination that commits
  per write (Airhouse).
- **Previews refuse this kind:** a sample would create report jobs in the
  advertiser's account.

## Credentials: per-kind `*_var` keys

A `*_var` key names a secret **key**, never a value; the run resolves it from
the secret manager, writes the value under the plain name (`client_secret_var`
→ `client_secret`), and removes the `_var` key. **The sets are per kind and do
not overlap — read the row for your kind, never borrow from another:**

| `kind` | `*_var` keys it manages |
|---|---|
| `toast` | `client_id_var`, `client_secret_var` |
| `quickbooks` | `client_secret_var`, `refresh_token_var`, `client_id_var` — or `access_token_var` alone (read-only custody) |
| `sp_api` | `client_secret_var`, `refresh_token_var` |
| `amazon_ads` | `client_secret_var`, `refresh_token_var` |
| `netsuite` | `private_key_var` — a multi-line PEM; the secret store must keep its newlines |
| `clickhouse` | `password_var` |
| `sql_database`, `postgres_cdc` | `connection_string_var` (a whole DSN) |
| `weather`, `besttime` | `api_key_var` |
| `google_sheets` | `service_account_json_var` |
| `rest_api` | nested under `config.auth`: `token_var` (bearer) or `key_var` (`api_key` header or query) |

Any other kind manages no credentials.

- **Borrowing across rows fails at run time, not at parse time.** Put
  `refresh_token_var` on a `toast` config, or `client_id_var` on an `sp_api`
  config, and nothing resolves or removes it; it reaches the connector's strict
  config and the run dies on `unknown field` — in a file that compiled cleanly.
- **The `_var` form is valid only in the YAML.** The connector never accepts
  `password_var` and the like; it only ever sees the resolved plain field.
- **An unresolvable secret name fails the run and names the secret. A secret
  that resolves to an empty string counts as unset** — the field is left out
  entirely (ClickHouse then sends no password at all, not an empty one).

## Run semantics

- **Partial failures are their own status.** A run that loaded some resources
  and failed others records as **`completed_with_errors`**. A dashboard or
  alert that only matches "failed" reports it as a clean success.
- **Schema migration is additive-only, structurally.** A run can add a table, a
  column, or a *variant* column for a type conflict (the old column is left
  alone); it can never drop or narrow a column in place. **A pipeline sitting
  on a wrong schema never self-heals, however many times you rerun it.**
- **Two recoveries, and they are not interchangeable:**
  - **Reset schema** drops the pipeline's destination tables and clears its
    cursors, so the next run re-infers a fresh schema and re-pulls everything.
    Destructive on purpose.
  - **Rewind cursors** (all of them, or one resource's) keeps every table and
    row and only moves the cursor back, so the next run re-pulls a window. It
    is **refused** where re-pulling would duplicate rows — an append-only
    table, or a merge without a key — unless explicitly forced. A
    `replace` table, or a keyed merge, converges safely.
- **A retry resumes from the cursor; it does not re-pull history.** Cursors
  are kept per workspace and pipeline, and advance only after a load commits,
  so ordinary retries are gap-free. A backfill run keeps its own resumable
  window and is retried in place rather than cloned.
- **A `modified`/`updated_at` cursor can lie.** Some sources change a record
  without advancing its modified timestamp (Toast does not advance it when a
  card payment is captured), so an incremental run never sees the change. A
  backfill meant to repair that must widen **both** ends of its window — not
  only the start — or it re-reads the same stale rows.

### Duplicate rows in a `replacing` table

`write_disposition: replacing` never writes the served table directly — it
appends to a buffer in a sibling *schema*, `<schema>_raw.<table>`, that a
merge-on-read fold rebuilds latest-wins (chosen to avoid a full-table
`MERGE INTO` that runs the data plane out of memory). **That fold runs twice
over: inline at the end of every successful load, and again on the scheduled
warehouse vacuum, from identical SQL.** Either can be the one that promotes a
given row.

That shape is where duplicates come from: **two folds whose snapshots overlap
each purge against a base the other has not committed yet, so both versions of
a changed row survive.** Measured, not hypothetical — dozens of excess rows in
a production orders table, **every duplicate pair spanning two
`_aw_load_id`s**. That span is the diagnostic, so check it *before* auditing
your own config: a pair straddling two load ids is this, not your config — and
if it does not straddle, this section does not explain your duplicates.

The guard is a **single-flight lease**: at most one active run per
(workspace, pipeline), taken atomically in the database so that replicas
racing each other cannot both win. Two consequences:

- **`allow_concurrent_runs: true` turns the guard off.** Set it only on a
  pipeline that owns no cursor and writes no `replacing` table; on a
  `replacing` pipeline it re-admits exactly this bug.
- **A dead-lettered run never releases its lease.** The lease's 6-hour expiry
  is a crash backstop, not a reclaim policy, so a pipeline stuck "busy" with
  nothing running is this gap — ask an operator to release the lease rather
  than waiting six hours.

The lease stops *new* overlapping folds; being an admission guard, it cannot
repair rows that already landed. Those need **Reset schema**.

## Traps

**A `# yaml-language-server: $schema=` header on a pipeline is dead, and its
failure is silent.** Some real pipelines open with one pointing at a
`json-schemas/pipeline.json` on GitHub. That URL returns **404**, and no JSON
schema for `.airway.yml` is published anywhere. Start from such a file and you
inherit an editor integration that validates nothing, warns about nothing, and
looks like it works. Delete the line.

**Two `clickhouse` source footguns, both silent.** (a) `port: 8443` with no
`secure:` silently uses plaintext on a TLS port — tolerable only because the
setup wizard emits both keys together, and these are hand-edited files. State
`secure:` explicitly and match it to the port (`8123`/`false`,
`8443`/`true`). (b) Socket timeouts belong in the source's `settings:` map,
never in a SQL `SETTINGS` clause: ClickHouse fixes the response socket's
timeout at request setup, *before* parsing the query body, so an inline
`SETTINGS http_send_timeout=…` is accepted and ignored — exactly what someone
reaches for when tuning a slow mirror.

**Comments inside pipeline files do not self-update.** One real pipeline still
carries a header saying its connector "has not landed" and the engine must be
pinned to a build registering that `kind` — long after it was wired, with
nothing to flag the comment as stale. Treat every in-file claim about what is
supported, this page included, as dated; the run is the authority.
