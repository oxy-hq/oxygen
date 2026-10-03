---
name: oxy-cli
description: Use when you need data out of an Oxy deployment — querying a customer's warehouse or semantic layer, reading threads/runs/apps/orgs, checking why an endpoint 4xxs, or reproducing a customer-reported bug against real data. Also when you need to know which endpoints exist or what body one takes. Triggers on "query the customer's data", "what does this API return", "debug this in production/dev", "which endpoint", "check the thread/run/app", "oxyc", "oxy api".
---

# Getting data out of Oxy with `oxyc`

`oxyc` is an authenticated `gh api`-shaped client. Prefer it over `curl` — it
resolves the token, picks the right credential for the surface, and can tell
you what endpoints exist.

```bash
# Not published to npm yet, so `npx @oxy-hq/cli` 404s. From a checkout:
node <repo>/sdk/cli/dist/main.mjs <command>
```

## The loop that works

**Never guess a path.** Three commands, in this order:

```bash
oxyc routes sql                            # 1. what exists, and what it does
oxyc schema {workspace}/sql/query          # 2. the body it expects
oxyc api {workspace}/sql/query \
  -f 'sql=select 1' -f database=<name> --md   # 3. call it
```

To get data out of a customer's warehouse, that is the whole path:

```bash
oxyc api orgs --md                                  # org ids
oxyc api {org}/workspaces --md                      # workspace ids
oxyc api {workspace}/databases --jq '.[].name'      # connection names
oxyc api {workspace}/sql/query -f 'sql=…' -f database=… --md
```

`oxyc routes <filter>` matches on method, path, surface and description, so
`oxyc routes query`, `oxyc routes admin`, `oxyc routes semantic` all work. Run
it with no filter only when you genuinely want all ~670 — it is a lot of
context.

## Placeholders — do not hunt for ids

`{org}`, `{workspace}`, `{project}`, `{customer}` and `{me}` fill themselves
from the customer repo you are standing in, or from `--org` / `--workspace` /
`--project`.

```bash
oxyc api {org}/workspaces --md      # find a workspace id
oxyc api {workspace}/agents
```

An unresolved placeholder errors and names the flag that would fill it. It is
never sent to the server as a literal.

## Keep the context small

- `--jq '<expr>'` on the server-side shape, **always**, before you look at a
  large response. Dumping a full thread list into context is the common waste.
- `--md` turns an array of objects into a markdown table — far fewer tokens
  than the same rows as JSON, which repeats every field name per row.
- `--cache 5m` when you are walking the same endpoints repeatedly.
- `--paginate` merges every page into one document. It is a heuristic on this
  API (pagination is not uniform); if a result looks short, pass
  `--paginate-key <field>`.

## Beyond the API

    oxyc validate                  # check the workspace YAML — no network, no token
    oxyc proxy --env dev           # local app dev against cloud data
    oxyc guide                     # this page, to paste into a context file

`oxyc validate` is the one command that works entirely offline. It checks
`config.yml`, `.automation.yml`, `.agentic.yml`, `.app.yml` and
`.agent.test.yml` against the schemas the Rust config types generate. It is
STRUCTURAL only — `oxy validate` also resolves `databases:` and `llm.ref`, and
wins where the two disagree.

If your runtime speaks MCP, `oxyc mcp` serves the same API surface as four
tools (`oxy_routes`, `oxy_schema`, `oxy_request`, `oxy_whoami`) instead.

## Custom apps: what is live, healthy, used

Read-only — every request is a GET. `<app>` is `<org-slug>/<app-slug>` or an
app UUID; each command takes `--json`.

    oxyc apps list [--org <slug>] [--published|--draft] [--builds]   # every app you can see
    oxyc apps show <app>           # live build, draft build, health, availability, 7-day usage
    oxyc apps builds <app>         # build history; which build is live, which is the draft
    oxyc apps health [<app>]       # fleet table (--needs-attention), or one app's checks and errors
    oxyc apps usage <app>          # views, visitors, tracked events over 7 days
    oxyc apps drift [<app>]        # commits in a LOCAL checkout after the live build's commit

- Use these rather than `oxyc api api/customer-apps…` by hand: they walk the
  paged listing to its end and resolve `<org>/<app>` to the id the routes take.
- **A failed request never prints as an empty result.** The part that was not
  read says `NOT READ` and the exit code is non-zero. A failing health check or
  a drifted app is a report and exits `0` — read the output, not the code.
- **`drift` answers `in sync`, `N commits ahead`, or `unknown — <reason>`.**
  `unknown` means the comparison could not be made (commit not recorded,
  repository not checked out, commit not fetched, no matching `oxy-app.json`,
  uncommitted changes) and is NOT evidence of no drift. It never fetches: run
  `git fetch` in the checkout first if you want the remote's state.

## Branch on the exit code, not the text

| | |
| --- | --- |
| `0` | fine |
| `2` | you called it wrong — fix the command, do not retry |
| `4` | not authenticated → `oxyc login --env <env>`, or the token expired |
| `5` | 404 — check the path with `oxyc routes`. In an **admin** surface a 404 can be a scope boundary, not a missing row. |
| `6` | the request was malformed — check `oxyc schema` |
| `7` | 5xx / timeout / network — **retryable** |
| `8` | refused: the operation would have destroyed something |

## Traps specific to this API

- **`200` with a body of `null` can mean an expired session**, not "no such
  thing" — `/api/user` does exactly that. `oxyc` warns on stderr when it sees
  one; `oxyc whoami` tells the two apart.
- **`oxyc schema` covers the data plane, not everything.** The document is
  curated for exactly the endpoints you need to build a body for — SQL, semantic
  query, and the lookups that resolve ids. A blank schema means *undocumented*,
  not nonexistent: `oxyc routes <path>` confirms the endpoint is real and shows
  what the handler says it does.
- **`/sql/query` does not return an object by default.** It returns arrays of
  strings, **header row first** — `[["id","name"],["1","ada"]]`. `--md` renders
  that as a table; `--jq '.[1:]'` skips the header. Only `result_format:
  "parquet"` returns an object, and only that one carries `truncated`.
- **A listed route can still 404** if it is `ide-only` or `worker-only`; those
  are hidden by default and shown by `oxyc routes --all`.
- **Pick the environment deliberately.** `--env local|dev|staging|production`
  (default production), or paste a URL: `--env https://poke-house.oxygen-hq.com`
  targets that deployment *and* sets `{org}`.
- **Staff hitting a tenant surface need an assume-role session.** A 403 there
  usually means no active session, not a mis-modeled role — check with
  `oxyc assume status`, which answers "none" without failing.

```bash
oxyc assume start --org <slug|uuid|url> -r "why"   # 60 min, not renewable
oxyc assume status                                 # what is live, minutes left
oxyc assume end                                    # or --all
oxyc login --login-env dev,staging                 # log into several at once
```

The session hangs off your **account**, not this terminal — your browser is in
there too, and `end` gets you out of both.

## Do not

- Do not `curl` the API by hand — you will get the credential wrong for
  `/external/api/**`, which takes `X-API-Key` rather than a bearer.
- Do not run a mutating request (`-X POST/PATCH/DELETE`) against **production**
  on your own initiative. Read freely; ask before you write.
- Do not paste a token into a command line or a file. `oxyc` reads it from the
  login cache; `oxyc token` exists if something really needs it.
