# Reading data while vibe-coding: `oxyc`

You don't need an `X-API-Key` to poke at the data API by hand — a bearer
token works, and **`oxyc`** (`npm i -g @oxy-hq/cli`) is the client that sends
it for you. **Every command that talks to a deployment's HTTP API is `oxyc`**;
the `oxy` binary has none. It reads the same `credentials.json` an older
`oxy login` wrote, so an existing login still works.

The bearer is `OXY_TOKEN` (or the variable `--token-env` names), else the
`oxyc login` cache for that deployment.

## As an agent: your own agent token

**An agent does not read data on its engineer's login.** It mints its own
token for the task:

```bash
eval "$(oxyc tokens create --agent --env <deployment>)"   # prints: export OXY_TOKEN=oxy_pat_…
oxyc api user --env <deployment> --token-env OXY_TOKEN     # …work…
oxyc tokens revoke --current --env <deployment> --token-env OXY_TOKEN   # last
```

- A page opens in your engineer's browser; **the click is theirs** — tell
  them it is waiting, and never click it with a browser tool. The token is
  printed once as `export OXY_TOKEN=…` and never written to disk; never put it
  in a file, a commit or your report.
- It reaches what its approver can reach, for 8 hours
  by default (`--hours`, 1 to 168). Staff or partner access comes only with
  `--standing`, and the approver still chooses it on the page.
- Pass `--token-env OXY_TOKEN` to every command: one that lost the variable
  then exits `4` instead of running on a cached login.
- It is not the sandbox agent token. To build and test a custom-app change,
  mint `--sandbox-agent` instead ([sandbox-agent-loop.md](sandbox-agent-loop.md)).
  A sandbox agent token is refused by `oxyc api` before any request (exit `2`),
  and the data API answers it `404`: read what your change did through your
  sandbox (`oxyc fn call`, `oxyc invocations`, `oxyc logs`). Do not unset
  `OXY_TOKEN` to reach the login cache.

## Ask the deployment, never guess a path

The point of the tool is that **you never guess a path** — ask the deployment
you are actually talking to:

```bash
oxyc login --env local       # a person, once; caches a token per host

oxyc routes projects --env local                        # what exists, and what each does
oxyc schema projects/{project_id}/query -X POST         # the body it expects
oxyc api user --env local                               # who am I?
oxyc api projects/<projectId>/query --env local -f sql='select 1' --md
```

- **Path** is relative to `/api/` (`oxyc api user` → `<target>/api/user`); a
  leading `/` or `api/` is fine, and `/external/api/**` passes through to the
  API-key surface untouched.
- **Fields follow `gh api`**: `-f k=v` is a **string** field, `-F k=v` a
  **typed** one (`true`, `3`, `["a"]`, `@file`), `--input @body.json` (or `-`)
  a raw body. On GET/HEAD/DELETE, fields become query parameters instead.
  `-X` sets the method (POST when a body is given); `-i` includes the status
  line and response headers.
- **Read cheaply.** `--md` renders a markdown table and `--jq '<expr>'` (`-q`)
  filters before you pay for the rows — both far fewer tokens than raw JSON,
  which repeats every field name per row. `/sql/query` answers arrays of
  strings **header row first**, not objects; `--md` is what makes that
  readable.
- **Exit codes are a contract** (`oxyc exit-codes`) — branch on them rather
  than parsing stderr: `4` = no usable credential, `5` = not found, `6` = the
  request was malformed and will fail identically forever, `7` = retryable.
- **Credentials are per deployment and `--env` defaults to production.** A
  401 that reads like an expired session is usually a dev call made with a
  prod token — check the `--env`. A `200` with a `null` body is the *other*
  expired-token shape; `oxyc whoami` tells them apart.
- For a raw `curl`, `oxyc token` prints the resolved bearer:

  ```bash
  curl -H "Authorization: Bearer $(oxyc token --env local)" \
       -H 'content-type: application/json' \
       localhost:5173/api/projects/<projectId>/query -d '{"sql":"select 1"}'
  ```

`--env` resolves the same targets as `oxyc publish` (`local` → the Vite dev
server `:5173`, which proxies `/api` → oxy `:3000`; `dev`, `staging`,
`production`), and a value with a `.`, `/` or `:` is read as a URL.

Access is your **normal org membership** — fine for your own test workspace,
not a cross-customer backdoor. The `projectId` is the one the vite-plugin
already resolved for the app (it's in `window.__OXY_APP__`).

## Working locally against cloud data: `oxyc proxy`

`pnpm dev` is cross-origin, so the browser sends no oxy cookie and the data
API answers `origin not allowed`. The sidecar listens where a local
`oxy serve` would (`:3000`) and forwards your dev server's Oxy calls to the
cloud target with your token attached:

```bash
oxyc proxy --env dev          # then `pnpm dev` as usual
```

Its guardrails are the reason to use it rather than a bare token:
side-effecting calls are **held** (`--allow-writes` forwards them), tracking
events **dropped** (`--allow-events`), and a production target is refused
without `--yes`. The token stays in the proxy process and never reaches the
browser. It cannot target a sandbox.
