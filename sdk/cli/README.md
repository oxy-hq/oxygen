# `@oxy-hq/cli`

`oxyc` — a `gh api`-shaped client for the Oxy HTTP API, plus the tooling that
manages customer workspace repos.

- **API client** — `api`, `routes`, `schema`, `openapi`, `login`, `whoami`, `token`, `login-link`, `tokens`, `assume`, `oltp`, `airhouse`
- **Experiments** — `experiments` (draft, price, register, status, interim, abort, result)
- **Feature flags** — `flags` (list, show, set, rules, eval, org)
- **Home layout** — `home` (show, get, apply, reset)
- **Customer workspaces** — `list`, `new`, `import`, `doctor`, `update`, `adopt`, `launch`
- **Your workspace's repository** — `workspace clone`
- **Custom apps** — `publish`, `init-ci`, `proxy`, `apps`
- **Checks** — `checks run`
- **Sandboxes** — `env`, `fn dev`, `fn call`, `invocations`, `logs`
- **Workspace previews** — `preview`
- **Development** — `validate`, `mcp`, `guide`, `skills`

## Install

```bash
npm install -g @oxy-hq/cli      # then: oxyc
npx @oxy-hq/cli routes          # zero-install; works for everything but `skills install`
```

From a checkout:

```bash
cd sdk/cli && pnpm install && pnpm build
node dist/main.mjs --help
pnpm link --global              # or, for `oxyc` on PATH
```

Requires `node >= 20`. `gh` (authenticated) is required for the customer
commands, `git` for the repo commands. Each is reported by name with its
install line the first time a command needs it. `--jq` needs nothing extra. It
uses `jq` when it is on PATH and an embedded copy of jq when it is not, as
`gh api -q` does.

## Quick start

```bash
oxyc login --env dev                        # browser flow (a URL + code where there is no browser); stores a token
oxyc whoami --env dev                       # verify it against the deployment

oxyc routes threads                         # find the endpoint
oxyc schema {workspace}/threads -X POST     # find the body it takes
oxyc api {workspace}/threads --md           # call it
```

## Global flags

Accepted by every command except `validate`, `guide`, `exit-codes`, `skills`
and `cache`.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--env <name\|url>` | `production` | deployment to target |
| `--target <url>` | — | explicit base URL; overrides `--env` |
| `--token-env <VAR>` | `OXY_TOKEN` | env var holding the bearer: any credential. **Named, it is the only source** — an unset variable is exit `4`, never a fallback to your login |
| `--api-key-env <VAR>` | `OXY_API_KEY` | env var holding the legacy API key or API token for `/external/api` |
| `--service-account <id>` | `OXY_SERVICE_ACCOUNT` | in GitHub Actions: the **ID** of the service account the OIDC exchange acts as |
| `--org <slug>` | — | value for the `{org}` placeholder |
| `--workspace <id>` | — | value for the `{workspace}` placeholder |
| `--project <id>` | — | value for the `{project}` placeholder |
| `--customer <name>` | — | act as though run inside this customer's repo |
| `--quiet` | — | suppress progress messages on stderr |

### Environments

`--env` takes a name or any URL. Anything not in this table is used as a URL.

| `--env` | URL |
| --- | --- |
| `local` | `http://localhost:5173` |
| `dev`, `development` | `https://aip.dev.oxy.tech` |
| `staging` | `https://aip.staging.oxy.tech` |
| `production`, `prod` (default) | `https://app.oxygen-hq.com` |

Credentials are stored **per deployment**, and the default is production. A
401 against dev usually means you are logged into prod — pass `--env` to
`login` and to the call.

## `oxyc api`

```
oxyc api <path> [flags]
```

`<path>` is relative to `/api`; a leading `/` or `api/` is accepted. One
exception: `customer-apps/…` is sent as written, because `/customer-apps/<org>/<app>/…`
is where an app's bundle is served. The app registry API is `api/customer-apps/…`
— spell the `api/` out for it (a 404 on the bare form says so).

| Flag | Meaning |
| --- | --- |
| `-X, --method <verb>` | HTTP method. Default GET, or POST when a body is present. |
| `-f, --raw-field <k=v>` | string parameter |
| `-F, --field <k=v>` | typed parameter — `true`, `3`, `["a"]`, `@file`, `@-` |
| `-H, --header <name:value>` | extra header (repeatable) |
| `--input <file\|->` | raw request body from a file, or `-` for stdin |
| `-q, --jq <expr>` | filter the response through jq (the `jq` binary, else an embedded jq) |
| `--md` | render the result as a markdown table |
| `--paginate` | follow every page and return one document |
| `--paginate-key <field>` | the field holding the rows, when the guess is wrong |
| `--max-pages <n>` | stop after `n` pages (default 100) |
| `--slurp` | with `--paginate`, emit an array of pages instead of merging |
| `--cache <duration>` | reuse a recent successful GET (`30s`, `5m`, `2h`) |
| `-i, --include` | print the status line and response headers |
| `--silent` | make the request, print nothing |
| `--verbose` | log the request before making it |
| `--timeout <duration>` | request timeout (default `2m`) |

**Bodies.** `-F 'ids[]=a' -F 'ids[]=b'` accumulates into an array. On GET, HEAD
and DELETE, fields become query parameters instead of a body.

**Placeholders.** `{org}`, `{workspace}`, `{project}`, `{customer}` and `{me}`
are resolved from the customer repo you are standing in, from an org URL passed
to `--env`, or from `--org` / `--workspace` / `--project`. An unresolvable
placeholder is an error naming the flag that fills it — never a literal sent to
the server.

**Surfaces.** The path selects how the credential is sent. `/api/**` sends it
as a bearer — whichever one [resolves](#authentication). `/external/api/**`
sends `X-API-Key` from `$OXY_API_KEY`, which holds a legacy API key (`oxy_…`) or
an API token (`oxy_pat_…`, `oxy_sat_…`, `oxy_ci_…`). With none set, an
`OXY_TOKEN` holding either of those stands in for it, so one `OXY_TOKEN` covers
both surfaces. A session token never does.

**`--paginate` is a heuristic.** Oxy sends `Link: rel="next"` on some endpoints
and nothing on others; where there is no header, `oxyc` reads `next_offset`
(and asks again with `?offset=`), then `pagination.has_next`, then `has_more`,
then `page < total_pages` (asking again with `?page=`). Nothing recognised
means one page, and it warns when the server said nothing at all.

```bash
oxyc api {org}/workspaces --md
oxyc api {workspace}/threads -q '.threads[].title'
oxyc api {workspace}/sql/query -X POST -f 'sql=select 1' --md
oxyc api /admin/users --paginate --md
```

## Discovery

```bash
oxyc routes [filter] [--json] [--all] [--refresh]   # endpoints this deployment mounts
oxyc schema <path> [-X <verb>]                      # request/response shape for one
oxyc openapi                                        # the whole OpenAPI document
```

`routes` is served by `GET /api/_catalog`, so it describes the deployment you
are talking to rather than a baked table. It is cached per host; `--refresh`
re-asks. `--all` includes ide-only and worker-only mounts. `--json` adds each
route's surface and fleet role.

## Asking an agent

```bash
oxyc ask <workspace-id> <agent> "How many orders last week?" [--thread <id>] [--wait <s>] [--json]
```

One request: the server waits up to `--wait` seconds (1 to 45, default 45) for the
answer, and the answer goes to stdout so it pipes. It is the route a custom app's
`useAskAgent` calls, as the person logged in.

- **The agent asked a clarifying question:** the question and its suggestions go to
  stdout, exit `0`; answer with `oxyc ask … --thread <id> "<answer>"`.
- **Still running when the wait runs out:** exit `0`. The ask keeps running on the
  fleet, and stderr names the run and the thread to read it in. `--json` carries
  `status: "running"` for a caller that branches.
- **The ask failed, timed out or was cancelled:** exit `1`, with the error.

## Authentication

```bash
oxyc login [--env <e>] [--login-env <e...>] [--assume <org> -r <why>] [--browser | --no-browser [--approver <email>] [--no-wait]]
oxyc whoami [--json]        # who the credential is, and what it can reach
oxyc token                  # print the bearer, for a raw curl
oxyc login-link [--next <path>] [--json] [--open]   # a one-time URL that signs a browser in
oxyc logout                 # revoke the cached token, then forget it
oxyc tokens list [--json]   # your personal access tokens
oxyc tokens revoke <id>
oxyc tokens revoke --current   # end the token in OXY_TOKEN
oxyc tokens create          # opens Account → Personal access tokens
oxyc tokens create --sandbox-agent --app <org>/<app> [--app …] [--hours 8] [--name …] [--browser | --no-browser [--approver <email>]]
oxyc tokens create --agent [--standing] [--hours 8] [--name …] [--browser | --no-browser [--approver <email>]]
```

**An agent runs on its own token, never on the person's `oxyc login`.** Which
one depends on the task:

| The agent is… | It mints | Which reaches |
| --- | --- | --- |
| building a custom app in a sandbox | `tokens create --sandbox-agent --app …` (`oxy_sbx_…`) | the sandboxes of the apps it names, and nothing else |
| doing anything else: reading data with `oxyc api`, reproducing a bug against live data | `tokens create --agent` (`oxy_pat_…`) | everything its approver can, for hours |

Both are approved once in the browser by the person the agent works for, both
are printed once as `export OXY_TOKEN=…` and never stored, and both end with
`tokens revoke --current`. After a mint, pass `--token-env OXY_TOKEN` to every
command: a named variable is the only source, so a command that lost the
variable exits `4` instead of quietly running on the person's cached login.

**Every command resolves its credential the same way, first match wins:**

1. **`OXY_TOKEN`** (or the variable `--token-env` names). It holds any
   credential, and all are sent as a bearer: an API token — a personal access
   token (`oxy_pat_…`), a service account token (`oxy_sat_…`), a CI token
   (`oxy_ci_…`) or a sandbox agent token (`oxy_sbx_…`) — a legacy API key
   (`oxy_…`), a publish token, or a session token. A blank value counts as
   unset. An agent token is a personal access token by its prefix: only its
   own description tells it from a login (`oxyc whoami`). The value
   `from-proxy` means the token is not on this machine at all: see
   [A token the proxy adds](#a-token-the-proxy-adds).
2. **The login cache** — what `oxyc login` stored for this deployment.
3. **GitHub OIDC**, in a GitHub Actions job granted `id-token: write`: the job's
   OIDC token is exchanged, once per process, for a fifteen-minute token acting
   as a service account, and that token is revoked when the command ends. See
   [CI without a stored secret](#ci-without-a-stored-secret).

**Naming the variable stops the list at 1.** With `--token-env <VAR>`, and
always in `oxyc mcp`, an unset or empty variable is an auth error (exit `4`):
the login cache and the OIDC exchange are not tried. The flag says which
credential runs the command — an agent's scoped token, typically — so a typo in
the name must not quietly run it as whoever last ran `oxyc login` on the
machine. A person who wants `oxyc mcp` on their own login starts it with
`--login`. `OXY_API_KEY` is unaffected: a command that can use one on its own
(`checks run`, the sandbox verbs) still does.

`OXY_API_KEY` holds a legacy API key or an API token, and is honoured where it
always was — the `/external/api` surface, and `checks run`.

**Legacy API keys and API tokens are separate things.** A legacy API key is the
older `oxy_…` key: it reaches everything its owner can and can't be limited to
workspaces. It keeps working wherever it works today, but it is not a token —
`oxyc tokens` never lists or revokes one, and `oxyc whoami` calls it a
`Legacy API key`. They are extended and revoked in the web app, under
Settings → Workspace → Legacy API keys. Use an API token for anything new.

**`login`** runs a browser loopback flow. Against a current deployment it
receives a one-time code and exchanges it (PKCE) for a **personal access
token** named `oxyc on <hostname>`: revocable, a year, and replacing that
machine's previous one. No credential travels in a URL. Against a deployment
that predates the exchange it stores the session token the page hands back,
exactly as before, and says so — nothing needs upgrading first, in either
direction. `--login-env` is repeatable and comma-separated
(`--login-env dev,staging`); the browser opens once per environment, in
sequence.

**`logout`** revokes the cached token on the server, best-effort, then removes
it from this machine whatever the server said. It never touches `OXY_TOKEN`.

**`whoami`** makes a live call, so an expired or revoked credential fails here
rather than reading as fine from the cache. For a token it also prints what the
token is and what it can reach — `all access`, or one line per grant with the
role it is capped at, and any organization whose policy blocks it. For a legacy
API key it prints `Legacy API key`, and that it reaches everything its owner
can. For an agent token it says `agent token`, who approved it, whether it
carries their staff or partner access, and when it expires. When a token in
`OXY_TOKEN` is refused (exit `4`), the advice is never to log in — a login
cannot replace what is in the variable. An agent stops and reports.

**`token`** prints the bearer on stdout and nothing else. Inside a GitHub
Actions job with nothing stored it prints the token the job's OIDC identity
exchanges for — like `gh auth token` — and that one is **not** revoked on exit,
since the output is the token. It still expires in fifteen minutes.

**`tokens`** manages personal access tokens, and the server allows that from a
**browser session only**: a token that could list, mint or revoke tokens could
outlive its own revocation. Since `login` now stores a token, `tokens list` and
`tokens revoke` usually answer with a pointer to the web app (Account →
Personal access tokens, `<deployment>/?settings=account.tokens`); they work
from a login that stored a session token. `tokens create` always opens that
page — a new token's secret is shown once, and a terminal is the wrong place.

**`tokens create --sandbox-agent`** is the exception, and the one command an
agent runs to get its own credential. It mints a **sandbox agent token**
(`oxy_sbx_…`): the [sandbox loop](#sandboxes) on one to five named apps, in
sandboxes the token created, for 8 hours by default (`--hours`, 1 to 168) —
and nothing else on the deployment.

```bash
eval "$(oxyc tokens create --sandbox-agent --app acme/store --env dev)"
```

It opens the same browser loopback as `login`, with the apps and the lifetime
on the page; the agent's operator approves once, under their own session.
Stdout is the single line `export OXY_TOKEN=oxy_sbx_…`, printed once, and
everything else is on stderr. It holds no credential while it runs, never
writes the credentials file, and revokes no other token. `--app` is
`<org>/<app>` and repeats; arguments are checked before the browser opens
(exit `2`). Only staff who can open the app's sandboxes can approve.

**`--no-browser`** is the same approval for a machine no browser can reach
(a cloud container, a CI runner, a remote dev box), where the loopback's
`127.0.0.1` port is not the approver's. Instead of opening a page, `oxyc`
asks for a device code (`POST /api/auth/cli/device`) and prints, on stderr,
a URL (`<deployment>/cli-auth/device?code=…`) and an eight-letter code. The
operator opens the URL on any device where they are signed in, types the
code from the terminal (the page never shows it), and approves; `oxyc` polls
(`POST /api/auth/cli/device/poll`, with the PKCE verifier) and prints the
token exactly as above. A request lives ten minutes. A decline, or a refusal
on the page, stops the command with exit `4` (`access_denied`) and the
reason; nobody deciding in time is exit `7`. It works the same for
`tokens create --agent` and for `oxyc login`, with one difference: a token
approved by device code **never carries staff or partner access**, since
whoever started the request collects it and the approver cannot see who that
is. `--standing --no-browser` is a usage error (exit `2`), and a login made
this way reaches only what its approver's organization memberships do.

**`oxyc login` picks the device code itself where no browser can reach this
machine**, the way `gh auth login` prints a URL and a code: in a Claude Code
remote session (`CLAUDE_CODE_REMOTE`), a GitHub Codespace, over SSH (unless
`BROWSER` names an opener, as VS Code Remote-SSH does), and on Linux with no
`DISPLAY`/`WAYLAND_DISPLAY` (WSL excepted). It says which, on stderr.
`--browser` forces the loopback, `--no-browser` the device code, and
`OXYC_HEADLESS=1`/`0` does either without a flag. `tokens create --agent` and
`--sandbox-agent` decide the same way.

**On a CI runner** (`CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`) nobody can
approve anything, so `oxyc login` and `tokens create --agent|--sandbox-agent`
refuse at once with exit `4` instead of waiting ten minutes, and name what needs
no person: `OXY_TOKEN` from a CI secret, or a GitHub OIDC service account
(`--service-account`). Pass `--no-browser --approver <email>` to have a person
approve from their device anyway (an exported `OXY_APPROVER` counts with
`--no-browser`, `--no-wait` or in a Claude Code cloud session, never where oxyc
only detected SSH or a Codespace; on a runner only the flag lifts the refusal); `OXYC_HEADLESS` or `CLAUDE_CODE_REMOTE` also
lifts the refusal. The approval URL and code are printed even with `--quiet`.

**`oxyc login --no-wait`** is the device code for a caller that cannot watch a
command still running — an agent sees a command's output only when it ends.
It starts the request, prints the approval URL **alone on stdout** (the code
and the next step on stderr) and exits `0`. Once the person has approved,
**`oxyc login` again, with the same `--env`/`--target`**, collects that same
request in one poll and caches the token as any login does; before approval it
prints the URL again and waits. The request (device code and PKCE verifier) is
kept in `login-pending.json` beside the credentials file, owner-only, until it
is collected, refused or lapses (ten minutes). `--browser` skips it.
`--no-wait` is a usage error (exit `2`) with `--browser`, `--login-env` or
`--assume`; pass `--assume` to the `oxyc login` that finishes.

**`--approver <email>`** (env `OXY_APPROVER`, valid only with `--no-browser`;
anything else is a usage error, exit `2`) names who must approve, so nobody
has to hand them a URL: the platform tells that person itself, by push,
email and Slack DM where their organization connected it, and in their
Oxygen inbox. They type the code from your output on the approval page. The
notice never holds the code and links to `/cli-auth/device` without it, and
the request is then theirs alone: anyone else who finds the page is told
there is no such request. `oxyc` prints `Approval request sent to <email>`
beside the code and the URL; it prints that whether or not the deployment
knew the address, since the deployment never says. An address nobody holds
leaves a request nobody can approve: it lapses after ten minutes, so a typo
means running the command again with the right one. An agent still never
opens or approves the page.

What comes back is checked before anything is printed: it must be an
`oxy_sbx_`, described as `sandbox_agent` and not all-access, for exactly the
apps named, expiring no later than the hours asked for. Anything the check
cannot read — an app entry with no slugs, an expiry that is missing or not a
time — is refused rather than skipped. A deployment that predates sandbox
agent tokens would otherwise hand back an ordinary login token with the
approver's whole reach. On any mismatch the command prints nothing, revokes
what was minted, and exits `8`.

With that token in `OXY_TOKEN`, `oxyc` behaves differently in five ways:

- **An app resolves against the token's own list** (`GET /api/auth/token`),
  never the app listing (`GET /api/customer-apps`), which it may not call. An
  app it was not minted for is exit `5`.
- **Read-back uses `/api/customer-apps/{id}/…`** — `invocations list`,
  `invocations held` and `checks run` — the same routes every other credential
  uses; the token never reaches `/admin`.
- **Refused before any request, exit `2`:** an `--app-env` that is not
  `dev-<handle>`; `publish` without `--app-env`, or with `--promote`;
  `fn call`, `fn run`, `logs`, `invocations list` and `checks run` without
  `--app-env`; `env show` of production or staging; `env schedules`;
  `oxyc apps`, `tokens list` and `tokens revoke <id>`; and `oxyc api` against
  any path.
- **`whoami`** prints the token's description — `kind`, `expires_at`, the
  `apps` it reaches and its `minter` — and `--json` is that document as the
  server sent it.
- **`tokens revoke --current`** ends it (`DELETE /api/auth/token`). Exit `0`
  when it is revoked or was already dead. It works for any API token in the
  variable; a cached login is ended with `logout` instead.

A refusal from the server carries a hint an agent can act on — never "try
`oxyc login` again". Exit `4` under this token means it expired, was revoked,
or its minter lost access: stop and report.

**`tokens create --agent`** mints an **agent token** for everything that is not
a sandbox: a personal access token (`oxy_pat_…`) that reaches what its approver
reaches through their organization memberships, for 8 hours by default
(`--hours`, 1 to 168). It cannot be extended, edited or regenerated, and it
cannot create, extend or revoke tokens; it can open a browser session as itself
(`login-link`).

```bash
eval "$(oxyc tokens create --agent --env dev)"
```

The same browser loopback again, with the same guarantees: stdout is the single
line `export OXY_TOKEN=oxy_pat_…`, printed once, everything else is on stderr,
it holds no credential while it runs, never writes the credentials file and
revokes no other token. Arguments are checked before the browser opens (exit
`2`); `--agent` and `--sandbox-agent` together are a usage error.

`--standing` asks for the approver's staff or partner access as well. The
approver decides on the page, where it starts **off**: a token that comes back
without it is accepted, and stderr says so in one line. Without `--standing`, a
token that carries either is refused.

What comes back is checked before anything is printed: it must be a whole
`oxy_pat_`; it must describe itself (`GET /api/auth/token`) as `personal` with
`source` `oxyc_agent`; it must expire no later than the hours asked for; and it
must carry no standing that was not asked for. A deployment that predates agent
tokens would otherwise hand back an ordinary ninety-day login. On any mismatch
the command prints nothing, revokes what was minted, and exits `8`.

Credentials live in the OS config directory under **`oxy`** — the same file the
Rust `oxy login` wrote before it was removed, so an existing login still works:

- macOS: `~/Library/Application Support/oxy/credentials.json`
- Linux: `$XDG_CONFIG_HOME/oxy/credentials.json`

An entry may now also carry `token_id` and `expires_at`; both are optional, and
a file written before they existed loads unchanged. `OXY_CREDENTIALS_PATH`
overrides the path. Caches are separate, under `oxyc` (`~/.cache/oxyc`).

### A signed-in browser for an agent

Sign-in is passwordless — Google or GitHub OAuth, or a magic link — and an
automation agent can finish neither. **`login-link`** prints a one-time URL
that signs a browser in with the credential `oxyc` is running on, so the whole
sign-in is one navigation:

```bash
browser_navigate("$(oxyc login-link --env dev --next /ide)")     # Playwright MCP
```

```ts
// a Playwright script
const link = execFileSync("oxyc", ["login-link", "--env", "dev", "--next", "/ide"], { encoding: "utf8" });
await page.goto(link.trim());
```

Stdout is the URL and nothing else; what the link is goes to stderr.

- **The link works once and expires in minutes** (five, at the time of
  writing — the server's `expires_at`). The bearer is never in it: `oxyc`
  trades the token for a single-use ticket (`POST /api/auth/browser-ticket`),
  and the ticket and `--next` ride the URL's fragment, which is not sent to a
  server and so reaches no access log.
- **The session is bounded by the token.** It reaches what the token reaches
  and nothing more, lasts at most the `session_seconds` the server answers
  (stderr states it in hours), ends sooner if the token is revoked or expires,
  and cannot manage tokens.
- **It needs a personal access token** (`oxy_pat_…`) — what `oxyc login`
  stores. Any other credential is exit `4` with the next step: a session token
  from an older login, a service account, CI or publish token, a legacy API
  key. A sandbox agent token is exit `2`, before any request. A deployment that
  predates sign-in links is exit `5`.
- **`--next <path>`** is where the browser lands: a path on the deployment
  starting with a single `/`. A URL, `//host` or a backslash is exit `2`, and
  no ticket is spent on it.
- **`--json`** prints `{ "url", "expires_at", "session_seconds" }` instead.
  **`--open`** also opens the link in your default browser — which uses it, so
  the printed URL will not work a second time.

### A token the proxy adds

`OXY_TOKEN=from-proxy` says an egress proxy adds the credential to each
request on its way out, and `oxyc` sends none. Use it in a Claude Code cloud
environment whose Oxy token is a **network secret**: the token stays in
Anthropic's proxy, never in the VM, so neither the agent nor a log can read it.

1. In the cloud environment's settings, add a network secret: **Allowed
   websites** `app.oxygen-hq.com` (and `*.oxy.tech` for dev and staging),
   header `Authorization`, prefix `Bearer`, value a personal access token or a
   service account token.
2. In its environment variables, set `OXY_TOKEN=from-proxy`. It is not a
   secret.
3. `oxyc whoami --env <env>` prints who the secret is, and a `via` line naming
   the network secret.

What changes under the sentinel:

- **No `Authorization` and no `X-API-Key` is sent** by any command, and the
  login cache and GitHub OIDC are not tried. A host the secret does not list
  gets a request with no credential.
- **The token's kind is unknown here.** `oxyc` reads a kind off a token's
  prefix to choose a path before it asks (a sandbox agent token's limits, a
  service account's machine surface). Under `from-proxy` none of those apply,
  and the deployment decides. `whoami` asks the server, so it still reports
  the token; the sandbox loop needs its `oxy_sbx_` token in the variable.
- **A refusal names the secret, not `oxyc login`.** A 401 says to check the
  secret's hosts, header and prefix, and that the token may have expired.
- **`tokens revoke --current` refuses.** It would end the secret's token for
  every session in the environment; revoke it in the web app, then delete the
  secret.
- A token minted into `~/.config/oxy/agent-token.env` (the cloud hook's file)
  sets `OXY_TOKEN` before every command, so it replaces the sentinel.

### CI without a stored secret

In a GitHub Actions job granted `id-token: write`, with no `OXY_TOKEN` set,
**any** `oxyc` command signs itself in — **as the service account the workflow
names, by its ID,** in `OXY_SERVICE_ACCOUNT` (or `--service-account`). It asks
GitHub for an OIDC token whose audience is `oxy:<the deployment's host>`, posts
it with the account's ID to `/api/auth/oidc/exchange`, and uses the
fifteen-minute `oxy_ci_…` token that comes back, revoking it on exit.

The audience is **worked out from the URL `oxyc` is pointed at** — the host of
the resolved `--env` / `--target`, lowercased, with a port only when it is not
the scheme's default: `oxy:app.oxygen-hq.com`, `oxy:aip.staging.oxy.tech`. It is
never asked of the deployment, and nothing a server says can change it. So the
token a job gives to a host is good at the deployment that calls itself by that
host and at no other: one handed to staging cannot be replayed at production,
and a server cannot talk the job into minting a token for somewhere else.
There is no shared fallback, and plain `oxy` is never requested.

A deployment that answers to a different address than the one `oxyc` was
pointed at — reached through an alias or a port-forward — refuses with
`wrong_audience` and names the address it does answer to: point `--target` at
that. One with no public URL configured (`OXY_API_URL`) takes no GitHub
sign-in at all; use `OXY_TOKEN` there.

What makes the deployment honour the job is a
**trust policy** on that service account, naming the repository, the workflow
file and the job's `environment:` — registered by `oxyc init-ci`, or in the web
app under Organization settings → API access → Service accounts → *the
account* → Trusted access.

**The account is always named, never guessed.** Anyone can register a trust
policy that names a repository they do not own, so the deployment matches a
run against the policies of the one account its workflow asked for and no
other. With no account named, `oxyc` attempts no exchange at all: `publish` and
`checks run` go straight to the app's registered publisher (below), and every
other command fails with "not authenticated", saying `OXY_SERVICE_ACCOUNT` is
what is missing.

**By ID, never by `<org>/<name>`.** An org's slug can be changed, and once the
org renames or is deleted the slug is free for anyone — who could create the
same account name under it and register a policy on your repository's public
ids. A workflow that said `acme/deployer` would then be naming *their*
account. An ID never changes hands, so it is the only thing the deployment
takes: anything else answers `400 service_account_required`, and `oxyc` does
not fall back from that. The ID is shown in the web app (Organization settings
→ API access → Service accounts → *the account*), and `oxyc init-ci` writes it
for you. `oxyc` passes the value through untouched; the answer still says
`<org>/<name>`, so the log reads as it did.

```yaml
permissions: { id-token: write, contents: read }
jobs:
  deploy:
    environment: production          # the trust policy requires it
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20 }
      # or any oxyc command the grants allow
      - run: npx --yes @oxy-hq/cli publish --promote
        env:
          OXY_SERVICE_ACCOUNT: 3f2504e0-4f89-41d3-9a0c-0305e82c3301   # acme/deployer
```

Each command exchanges for itself, which is all a job running one command
needs. Several policies of the named account matching one run is not an error:
the oldest is used. The exchange is rate-limited per client address (60 a
minute); a `429` is waited out once, for its `Retry-After` (at most 60 s), and
retried before it fails.

The `setup-oxyc` action (`sdk/setup-oxyc`) does the exchange once for a whole
job: it installs `oxyc`, exports `OXY_TOKEN` for the later steps and revokes
the token in a post step. **It is not published yet** — `uses:
oxy-hq/setup-oxyc@v1` does not resolve, and a job naming it fails at "Set up
job" — so nothing here depends on it.

A refused exchange says what to fix: `no_matching_policy` prints the
repository, workflow, ref and event the run presented and where to register a
policy on the named account; `missing_environment`, `self_hosted_runner` and
`pull_request_target` name the rule that was not met. A deployment with no
exchange route (404) reads as "not authenticated", with that reason — except
in `publish` and `checks run`, which fall back to the app's registered
publisher, as they also do on `no_matching_policy` and when no account is
named.

## Acting as an organization

```bash
oxyc assume start --org <slug|uuid|url> -r "<reason>"
oxyc assume status [--json]
oxyc assume end [--org <slug>] [--all]
```

Sessions last **60 minutes and are not renewable** — re-running `start` returns
the existing session rather than extending it. `--org` is optional when `--env`
is itself an org URL. The reason is recorded in the audit log.

The session belongs to your account, not this terminal, so your browser shares
it and `end` ends it in both. A staff 403 on a tenant surface is usually no
active session rather than a role problem; `assume status` answers "none"
without failing.

## OLTP databases

Staff can see and request an org's transactional Postgres — the store behind
`ctx.oltp` — without a shell on the server.

```bash
oxyc oltp status [--org <slug|uuid|url>] [--json]
oxyc oltp provision --org <slug|uuid|url> --writer app:<slug> [--writer pipeline:<source> …] [--yes] [--json]
oxyc oltp provision --org <slug|uuid|url> --branch staging [--yes] [--json]
oxyc oltp reset --org <slug|uuid|url> --branch staging [--yes] [--json]
```

`status` lists every org, with a database or without; with `--org` it shows
that org's store and writers. No credential is ever printed.

`provision` prints what it will create and asks first, because it calls a paid
provider; without a terminal it refuses unless `--yes`. It is idempotent — an
existing database or writer is reconciled, not duplicated. `app:<slug>` is
derived the way the platform derives `ctx.oltp`'s schema (`app:store-ops` →
`app:store_ops`, schema `app_store_ops`); `pipeline:<source>` names
`raw_<source>`. A 404 can be an org outside your staff grant's scope, and an
active `assume` session closes `/admin`.

`--branch staging` also cuts the org's **staging branch**: one copy of the whole
database per org, which every app's staging environment writes to. It is made
only by this flag — never by a publish — and re-running it mints whatever a
writer added since the cut needs, without resetting anyone's data. `status`
shows its age and calls it stale past 30 days; nothing resets it on a timer.
`reset --branch staging` re-copies it from production, discarding every app's
staging data in the org, so it lists those apps and asks first (`--yes`
without a terminal).

## Airhouse tenants

Staff can see a workspace's Airhouse tenant — the warehouse behind
`ctx.airhouse` and `type: airhouse_managed` — and repair it, without the admin
console. The server provisions a workspace's tenant itself; `provision` is the
repair and backfill tool, for a workspace that predates that or whose tenant or
service account did not come up.

```bash
oxyc airhouse status [--workspace <id>] [--json]
oxyc airhouse provision --workspace <id> [--yes] [--json]
```

`status` lists every workspace, with a tenant or without; with `--workspace` it
shows that workspace's tenant and whether its service account is bound. The
fleet read is capped, and the cap falls on workspaces without a tenant, so a
workspace missing from a capped page is reported as possibly cut rather than
nonexistent. No credential is ever printed.

`provision` prints what it will create and asks first, because the tenant name
(`oxy-ws-<workspace id>`) is global and cannot be reused; without a terminal it
refuses unless `--yes`. It is idempotent — an existing tenant is reconciled
(a missing service account is bound), not duplicated. `--workspace` takes the
workspace's UUID. A 404 can be a workspace outside your staff grant's scope, a
503 is a deployment with no Airhouse configured, and an active `assume` session
closes `/admin`.

## Feature flags

Staff-only control of the flags Oxy declares in code
(`crates/app/src/server/feature_flags/registry.rs`): the kill switch and the
targeting rules — per-org / per-user overrides plus a weighted fallthrough for
A/B splits. Flags are not created at runtime. **Every write hits the
deployment `--env` names (production by default) and takes effect for real
orgs**; it needs platform-operate standing (an `assume` session does not give
it).

```bash
oxyc flags list                                   # key, kill switch, rules summary, unit, scope
oxyc flags show <key>                             # rules in full, and any unreadable-rules error
oxyc flags set <key> --on|--off [--yes]           # the kill switch only
oxyc flags rules <key> --set-org <org>=<variant> [--set-org …] [--yes]   # slug or UUID; `org=` removes
oxyc flags rules <key> --set-user <uuid>=<variant>
oxyc flags rules <key> --remove-org <org> | --remove-user <uuid>
oxyc flags rules <key> --everyone <variant>       # everyone not overridden
oxyc flags rules <key> --split control=50,treatment=50   # percent; must total exactly 100
oxyc flags rules <key> --file rules.json | --clear       # replace wholesale | back to the plain switch
oxyc flags rules <key> … --dry-run                # print the rules that would be sent
oxyc flags eval --org <org> [--user <uuid>]       # what each flag serves them: variant, reason, bucket
oxyc flags org <org>                              # what a member of that org sees (client flags)
```

`rules` reads the flag first and edits the rules it has, so adding one override
leaves the other overrides, the fallthrough and the salt as they were. A flag
with no rules starts from what it serves today (kill switch on is the second
variant, off the first). Percentages become basis points; a total other than
100%, or a share finer than 0.01%, is a usage error (exit 2) rather than a
rounded split. The override flags are `--set-org` / `--set-user` because `--org`
is the global "which org" flag. `eval` is a preview and records nothing. Rules
saved behind an off kill switch do not apply until `set <key> --on`, and the
command says so.

Every write asks first and refuses without `--yes` when stdin is not a terminal
(exit 8). A rejected rule set (422: an unknown variant, a split that does not
total 100%, rules on a flag whose read sites do not pass the org) is printed in
the server's words and exits 6. A flag whose stored rules this build cannot
read (409) is edited only by `--file` or `--clear`. Guide:
`internal-docs/feature-flags.md`.

## Experiments

An experiment's whole lifecycle, as records on the deployment: a draft design
edited in place, priced, registered (which freezes it and makes it the run),
watched, aborted or reported. Every verb takes `--workspace <id>` and `--json`.

```bash
oxyc experiments list
oxyc experiments design <name> > promo.yml          # the config, as YAML
oxyc experiments create --file promo.yml           # a new draft
oxyc experiments edit <name> --file promo.yml      # overwrite the draft
oxyc experiments duplicate <name> <new-name>       # how a registered one is changed
oxyc experiments price <name> --candidate '<yaml|json|@file>' [--candidate …] [--wait]
oxyc experiments proposals <name>
oxyc experiments use <name> <proposal-id>          # pricing's keys onto the draft
oxyc experiments register <name> [--yes]           # freeze it; it becomes the run
oxyc experiments retrospective <name> [--yes]      # past dates only: analyse as its one run
oxyc experiments status <name>                     # flips, guardrails, compliance, estimate
oxyc experiments abort <name> --reason <text> [--levers off|keep] [--yes]
oxyc experiments interim <name> [--refresh [--wait]]  # the reading so far, badged not final
oxyc experiments result <name>                     # sealed until reported
```

A candidate is one design to price, tagged by `shape`:
`{shape: waves, wave_sizes: [20, 20], first_switch_on: 2026-11-02, spacing_days: 14}`
or `{shape: switchback, first_period_on: 2026-11-02, period_days: 7, pairs: 6}`
(`blocks`, `assign_by` and `alpha` are optional). One to six per request;
`--wait` polls until each is priced, refused or failed — a refusal is the
answer, not an error. Lengths count periods of the design's `grain` (day by
default; hour, week, month), and every date must start one: a Monday at week
grain, the 1st at month grain, a local whole hour at hour grain
(`first_period_on: 2026-10-26T14:00`).

`edit` sends the hash of the draft it read (or `--base-hash`), and `register` /
`retrospective` send the hash of the draft they just read, so an edit someone
made in between is a 409 rather than a silent overwrite or a design registered
unseen. `register`, `retrospective` and `abort` ask first, and refuse without
`--yes` when stdin is not a terminal. `result` on a run that is not reported
prints that it is sealed and exits 0: there is deliberately no effect number
before `reported`. `interim` is the registered analysis over the periods
finished so far on a live or closing run, labelled not final (its interval is
the fixed-horizon one, so reading it early overstates confidence); `--refresh`
asks for a fresh one and a run not in progress exits 0 with its state. A proposed coefficient
(`write_back` in the result) is applied by editing the workspace's repository and pushing. Staff reach a tenant workspace only inside an
`oxyc assume` session.

## Home layout

The org's home is laid out by one document: an ordered list of sections. A `widgets`
section is a group (it may repeat, each with an optional `title`) holding an ordered list
of the widgets custom apps declare, each at a named size. An item may carry `params` (the
widget's settings), a `title` and an `id`, so one widget can be placed as several copies,
one per store, named `app/widget#id`. A widget's view may ask to be re-read with
`refresh_after` (seconds). `oxyc home` lets an agent see what a person sees, then change it.

```bash
oxyc home show --org <slug> [--json]               # the home as text
oxyc home get --org <slug> > layout.json           # { revision, layout }, to edit
oxyc home apply --org <slug> -f layout.json [--dry-run] [--revision N] [--force] [--json]
oxyc home reset --org <slug>                       # back to the default
```

`--org` takes a slug, UUID or org URL, and is optional when `--env` is an org URL.

`show` combines `GET /orgs/{id}/home` and `GET /home/widgets?org=`: the sections in
order, each widgets group under its title (`Widgets` when untitled) and, for each
copy, `app/widget[#id] · Title · size`, a `params: store=mission` line when it has
settings, `refreshes every 5m` when its view sets `refresh_after`, its headline with the
level as a word (`[attention]`), detail, `progress 61/84` or the metric, up to three rows
and its button labels, then `not shown:` for every declared widget the layout leaves
out (`(Title, needs settings: store)` for one that needs settings a layout has not
given). `--json` prints both answers raw as `{ home, widgets }`. `get` prints
`{ "revision": N, "layout": … }`: the `effective_layout` (always a complete document)
and the revision it was read at, so `get`, edit the layout, `apply` round-trips.

`apply` reads that file (or a bare layout) from a path or `-` (JSON or YAML) and sends
the revision **your edit started from**: the file's, or `--revision N`. A layout someone
saved after your `get` is therefore **exit 8** ("the home layout changed since it was
read") rather than an overwrite. A bare layout with no revision is refused unless you
pass `--revision` or `--force`, which sends none and overwrites. A layout the server refuses is **exit 6**, with every problem listed at
once as `path: message (fix)`. `--dry-run` prints the home the layout would give, as
`show` does, and saves nothing (live views come from the saved layout, so a widget it
does not yet draw reads "no live view until it is saved"). Writing needs an org owner
or admin; staff need an `oxyc assume` session. Widget `sizes` and the view a widget
returns are in the `oxy-custom-apps` skill.

## Customer workspaces

A customer **is** a GitHub repo carrying the `oxy-customer` topic. Tagging the
repo is the whole of registration; there is no separate registry file. These
commands need `gh` authenticated.

```bash
oxyc list [--json] [--refresh]        # the customers
oxyc path <customer>                  # where their repo is on this machine
oxyc new <customer> [--display <name>]        # create, tag and scaffold a repo
oxyc import <org/repo> [--clone]              # tag an existing repo
oxyc remove|rm <customer> [--purge --yes]     # untag (never deletes the repo)
oxyc doctor [<customer>] [--all]              # report state, change nothing
oxyc update <customer> [--apply] [--diff-all] # drift from the workspace template
oxyc adopt <customer> [--apply]               # install managed files an import lacks
oxyc activity <customer> [--since <date>] [--repo <org/name>] [--write] [--json]
oxyc launch <customer> [claude-args...] [--here] [--dry-run]
oxyc repos [--refresh]                # where OUR repos are checked out
```

`update` and `adopt` **report by default and write only with `--apply`**.
Neither commits, branches or pushes. What they may rewrite is decided by
`template/.oxyc-managed`: an unclassified file belongs to the customer and is
never touched.

`launch` starts a Claude Code session scoped to one customer; `--here` runs in
the current directory while granting access to the customer's repo.

## Cloning a workspace's repository

```bash
oxyc workspace clone <workspace> [--env dev|staging|production] [--dir <path>]
                     [--open code|cursor|none] [--branch <name>]
```

Authoring happens in the workspace's repository, not in the product: clone it,
edit with VS Code, Cursor or an agent, and push. **A push to the default branch
compiles and deploys.** The in-app "work in your editor" panel prints exactly
this command.

`<workspace>` is the workspace's id (`oxyc api {org}/workspaces --jq '.[] |
[.id, .name]'`), or its name when `--org <slug>` says where to look. The command
reads `git_remote_url`, `default_branch` and `repo_subdir` from `GET
/api/{workspace}/meta`, prints the plan and runs plain `git clone --branch
<default branch> <url> <dir>` with **your** git and credentials: no token is
injected and the URL is not rewritten. `--dir` defaults to the repository's
name and must not exist or be empty; `--branch` checks out something other than
the default branch. A workspace that lives in a subdirectory of its repository
(`repo_subdir`) is opened at that subdirectory.

`--open code` runs `code <path>` (the default when `code` is on PATH),
`--open cursor` runs `cursor <path>`, `--open none` launches nothing. **An
editor is only launched from an interactive terminal**: in a pipe, a CI job or
an agent's shell the command is printed instead, whatever `--open` says.

It ends with three lines on stdout: the path to open, `push to <default branch>
to compile and deploy`, and how to preview a branch first (`oxyc preview
create <branch> --workspace <id> --wait`, staff only; see [Workspace
previews](#workspace-previews)).

Exit codes: `5` the workspace has no repository (connect one in its Settings) or
does not exist, `6` `git clone` failed (git's own message is printed above it),
`8` the target directory is not empty, `2` a bad flag.

## Standing a customer up

Ten steps across three credentials. `bootstrap` is the runbook plus the part of
it `oxyc` can do; the whole sequence, with what each step is and how it fails,
is [`internal-docs/customer-bootstrap.md`](../../internal-docs/customer-bootstrap.md).

```bash
oxyc bootstrap plan   <customer> [--crews]            # the sequence; reads nothing, writes nothing
oxyc bootstrap run    <customer> --owner <email>      # do what oxyc can, print the rest
oxyc bootstrap verify <customer> [--json]             # did every step land?
```

Shared flags: `--name <display>`, `--repo <owner/repo>`, `--app <slug>`,
`--writer app:<slug>|pipeline:<source>` (repeatable), `--crews`,
`--phase <id>` (repeatable). `run` also takes `--owner`, `--dry-run` and
`--yes`.

The phases are `repo`, `org`, `billing`, `oltp`, `git-link`, `workspace`,
`airhouse`, `app`, `secrets` and — with `--crews` — `frontline`.

**`run` is safe to re-run**, which is the point: a half-finished standup is the
common case. It reads each phase before it writes and writes only what is
missing. It performs exactly two things — creating the org
(`POST /api/admin/orgs`) and provisioning or reconciling the workspace's
Airhouse tenant — and prints a checklist for everything else. It never
provisions the OLTP store (a billable project at a paid provider), never
provisions a subscription (Global Owner only), never sets a secret's value and
never enrols a worker.

**It works one surface at a time.** `/api/admin` is refused while an
`oxyc assume` session is live, and the GitHub import that gives a workspace its
remote is an org-scoped route — so a standup crosses between staff and tenant
credentials twice. `plan` prints the stage boundaries; `run` refuses rather than
half-writing when an assume session is open.

**Exit codes:** `0` every phase looked at has landed, `9` one has not (as
`checks run` means it), `8` refused because an assume session is live, `2` the
invocation was wrong — checked before any request.

```bash
oxyc bootstrap verify poke-house --env dev --json \
  | jq -r '.phases[] | select(.landed=="no") | "\(.id)\t\(.next)"'
```

## Custom apps

```bash
oxyc publish [--env <e>] [--dir <path>] [--promote] [--build-only | --prebuilt] [--json] [--allow-function-lint] [--app-env <dev-handle>]
oxyc promote <app> [--from staging|dev-<handle>] [--build <id>] [--remove-schedule <fn>]… [--dry-run] [--yes] [--json]
oxyc rollback <app> --app-env staging|production [--to <build>] [--remove-schedule <fn>]… [--dry-run] [--yes] [--json]
oxyc init-ci [--app <org>/<app>] [--environment <name>] [--promote] [--force] [--no-register] [--setup-action]
oxyc proxy [--app <org>/<app> --app-env staging|dev-<handle>] [--port <n>] [--allow-writes] [--allow-events] [--yes]
```

**`publish`**, from an app directory, runs `oxy-app.json`'s build (default
`pnpm install`, `pnpm build`, output `out/`), bundles each declared Oxy Function
with `pnpm exec esbuild` into `functions/<name>.js`, resolves the project from
the target's public `build-config`, and uploads the bundle — to the **draft**
channel (staging). `--env` defaults to **production**; name it. Ship it with
`oxyc promote`, below. `--promote` still publishes and promotes in one request,
through the same server-side path, and the server answers it with a
deprecation warning: it shows nobody the plan.

- **The manifest** is read strictly: `oxy-app.json` is validated against the
  JSON Schema `@oxy-hq/sdk` generates from its `OxyAppManifest` type (the same
  check `oxyc validate` and `@oxy-hq/vite-plugin` make, in the same words). An
  error — a wrong type, a malformed `slug` or function name, a missing
  required field, a notification catalogue the server refuses — stops the
  publish before the build with exit `2`, listing every problem; a field the
  schema does not know is a warning, since the platform ignores what it does
  not read.
- **Identity**: `--org` / `--app`, then `OXY_ORG` / `OXY_APP`, then the manifest's
  `orgSlug` / `slug`, then an `apps/<org>/<app>/` working directory. `--org`
  takes a slug or a UUID. `--project` pins the workspace (and implies its org).
- **First publish**: an app the target does not know yet makes the
  build-config lookup 404. With a stored token, `publish` lists the org's
  workspaces: exactly one, and it publishes there, saying so on stderr;
  several, and it exits `5` listing them with their uuids beside the
  `--project <uuid>` to pass. Under trusted publishing (no token) it exits `5`
  with the two-step hint, as before.
- **Kit versions**: an app whose `@oxy-hq/sdk` or `@oxy-hq/vite-plugin` is more
  than one minor behind the versions this `oxyc` was built beside gets one
  warning per package before the build, naming the `pnpm add`. Read from the
  app's `package.json` (the installed copy when there is one); no network.
- **`--dir`** publishes a pre-built directory instead of building; functions are
  still bundled into it. **`--build-only`** stops after building and bundling —
  no credential, no network unless the org must come from `--project`.
  **`--prebuilt`** (with `--dir`) skips esbuild and refuses if a declared
  function's `functions/<name>.js` is missing. The pair splits CI so the job that
  runs package scripts never holds the credential.
- **Auth**: `OXY_TOKEN`, then the login cache — or, in a GitHub Actions job
  with `id-token: write` and neither set, the job's OIDC token. (A variable
  named with `--token-env` is the only source: unset, the publish stops.) Two exchanges are tried, in order, and the exchange happens last, after
  the build. First the general one (`/api/auth/oidc/exchange`, audience `oxy:<the deployment's host>`):
  a **trust policy** on a service account (see `init-ci`). Then, only when the
  deployment has no such route (404) or answers `no_matching_policy`, the app's
  own **registered publisher** (`/api/customer-apps/publish/oidc-exchange`,
  audience `oxy-publish`) — the registration every workflow written before trust
  policies relies on; it needs the org **slug**. Any other refusal is final: it
  names something wrong with the run, which the older path would only hide.
- **Provenance**: the checkout's `origin`, `HEAD` and branch (else `GITHUB_SHA` /
  `GITHUB_REF_NAME`) are recorded; a missing half or a dirty app directory is a
  warning, never a failure. The build id defaults to
  `$GITHUB_SHA-$GITHUB_RUN_ID.$GITHUB_RUN_ATTEMPT`, else random — a reused one is
  a `409`.
- `.env.local`, then `.env`, are read from the directory and its parents without
  overriding the environment. The server's warnings print on stderr; `--json`
  prints its result on stdout.
- **Function lint**: before the build, each declared function's source (and the
  relative imports it reaches) is checked for what the host refuses at the first
  call — a `ctx.*` call whose capability the manifest lacks, a global the isolate
  does not have, a `ctx.warehouse` / `ctx.tx` write outside `destinations`; before
  the upload, with the target's database list, an `upsert` or `ctx.tx` on an
  engine that refuses it and a customer-warehouse write with no
  `customerWarehouseWrites` reason. The rules are `validate`'s, below. A finding
  fails the publish with exit `1` and names the file, line, call and fix; if the
  target will not list the databases, the engine half prints one warning and is
  skipped. **`--allow-function-lint`** is the way past a false positive: every
  finding becomes a warning that names its rule — please open an issue with it.

**`init-ci`** does two things: writes the workflow, and registers the trust that
lets it publish.

*The workflow* is `.github/workflows/oxy-publish.yml` at the repo root: a
`build` job (no id-token) that runs `publish --build-only`, and a `publish` job
bound to a GitHub `environment:` (`--environment`, default `oxy-publish`) whose
only work is `publish --prebuilt`. `build` installs pnpm with
`pnpm/action-setup@v6` — the pnpm the repo root's package.json pins
(`packageManager` or `devEngines.packageManager`), with no `version:` of its
own, since the action refuses a second, different one; pnpm 11 when nothing
pins it — and both jobs run the Node in `.nvmrc`, else 26 (pnpm 11 needs
Node 22.13 or later). It warns when nothing pins pnpm, and when
`pnpm-workspace.yaml` approves build scripts only with `onlyBuiltDependencies`,
which pnpm 11 ignores (the build's install would fail with
`ERR_PNPM_IGNORED_BUILDS`; move the entries to `allowBuilds`). The publish job uses no third-party action: each
step runs `npx @oxy-hq/cli@<this version>` and `oxyc` exchanges the job's OIDC
token for itself, acting as the service account `--service-account <org>/<name>`
names — by default `<app's org>/deployer`. **Here, and only here, the account is
given by name:** `init-ci` is run by a person, looks the account's ID up as that
person, and writes the ID into the workflow with the name beside it as a
comment (`OXY_SERVICE_ACCOUNT: 3f25…3301   # acme/deployer`). When it cannot
look the ID up — nobody logged in, no such account yet — it writes a
`<service-account-id>` placeholder and says so; it never writes the name as
the value. `--promote` makes the publish go live. **No `checks run` step is
written**, even when the manifest declares a check: the job's service-account
token is answered `403` on the check routes, which sit behind platform gates a
service account never passes, so the step could not succeed. The workflow
carries a one-line comment where it would be, and `init-ci` says so when it
writes one. Run the checks with a staff credential until `ci` tokens may.
`--setup-action` writes the same workflow around `uses: oxy-hq/setup-oxyc@v1`
instead, which exchanges once for the whole job. That action is not published
yet, so a workflow written with the flag cannot start until it is; `init-ci`
says so when it writes one.

*The registration* is a trust policy on that service account — this repository,
the workflow file, the environment — granted publishing this one app and
nothing else. `init-ci` creates the `deployer` account if none was named and it
does not exist, then the policy, and is idempotent: a second run finds both. A
named account is never created, and must be one of the app's own org — an
account's grants never reach another org, so `init-ci` refuses one. A service
account's name is lowercase words joined by single hyphens, starting with a
letter, 2–40 characters.

It can only register **when the caller can**: an org admin, logged in with a
browser-session credential — the server refuses those two creations to a token,
which is what a fresh `oxyc login` stores. When it cannot (not an admin, a token
login, offline, a deployment without trusted access, `--no-register`), the
workflow is still written and the command prints the exact steps instead: where
in the web app, the `oxyc api` calls with every id it managed to learn filled
in, and the older `…/publishers` registration for a deployment without trust
policies. Registration never fails the command.

**`--app-env <dev-handle>`** moves one sandbox's build pointer instead of the
draft/live channel — see [Sandboxes](#sandboxes). It needs a staff credential
(never a publish token or OIDC) and is refused together with `--promote`: a
sandbox build goes to production only through staging — `oxyc promote <app>
--from dev-<handle>`, then `oxyc promote <app>`.

**`promote`** moves a build up one environment: staging's to production (the
default), or a sandbox's to staging (`--from dev-<handle>`). Nothing else —
production is never promoted, and moving it backwards is `rollback`. It asks the
server for the **plan** first and prints it: the build (commit, branch,
publisher, how long the source has served it), what the target serves now,
functions and production schedules that change, the migrations production
applies **before** it moves, and `required` secrets production has no value for.
Then it asks on a terminal (off one it needs `--yes`, else exit `8`) and acts,
sending the plan's build and `expect_current` back — `409 target_moved` if the
target moved in between. `--dry-run` stops at the plan. A plan with blockers is
refused before any act (exit `6`): `staging_never_served` (production only
serves a build staging has served), `validation_not_passed`,
`required_secrets_unset`, and `schedule_removal_unconfirmed` — name each retired
schedule with `--remove-schedule <function>`. `--build` names an older build the
source served (publish id or UUID). Promoting the build already served is
`409 already_serving`.

**`rollback`** moves `--app-env` (staging or production; never a sandbox) back
to the build it served before its current one — twice swaps back — or `--to`
one it served earlier. Same plan, prompt and `--yes`. A rollback never reverses
a migration: the target's pending files are applied like any production move,
and the plan warns when the target predates files already applied. An unset
`required` secret only warns here (it is the emergency path).

Both need a staff credential: a publish token, a service account's and a sandbox
agent token are refused before any request (exit `2`). `--json` prints the
server's plan verbatim.

## Apps

Read-only except `domains add | verify | remove`: every other request these make
is a GET, against `/api/customer-apps/**` (the app-admin role). `<app>` is
`<org-slug>/<app-slug>` or an app UUID.

```bash
oxyc apps list [--org <slug>] [--published | --draft] [--builds] [--json]
oxyc apps show <app> [--json]
oxyc apps builds <app> [--json]
oxyc apps health [<app>] [--needs-attention] [--json]
oxyc apps usage <app> [--json]
oxyc apps drift [<app>] [--org <slug>] [--dir <path>] [--refresh] [--json]
oxyc apps domains [list] <app> [--json]
oxyc apps domains add <app> <domain> [--json]
oxyc apps domains verify <app> <domain> [--json]
oxyc apps domains remove <app> <domain> [--json]
```

- **`list`** — every app you can see, across organizations, walking the paged
  registry to its end. `STATE` is `live` when the app has a published build and
  `draft` when it has none. `SOURCE` is the repository the app is *registered*
  against. `--builds` adds the live build's id, the repository and commit it was
  built from, who published it and when — one extra request per app, six at a
  time. `--json` rows also carry the server's `verdict` and, for a published
  app that was measured, `availability`: request success over six windows and
  the burn verdict. An absent `availability` means "not read", never healthy.
- **`show`** — one app on one screen: the registry row, the live build, the
  draft build when it is newer than the live one, deployment integrity,
  availability, and the 7-day usage summary. `last promote` is when the promote
  or rollback action was last used; `oxyc publish --promote` does not update it,
  so it can be older than the live build. Availability is the registry row's
  reading, not a request of its own: with none, `show` prints `no reading`, and
  it is never in `failed`. `--json` has the registry row under `app` and its
  `availability` (or `null`) under `availability`.
- **`builds`** — the build history, newest first. `CHANNEL` marks the build each
  channel points at: `live`, `draft`, or both for one build.
- **`health`** — with no argument, the fleet table (published apps only):
  `down`, `degraded`, `not_measured`, `quiet` or `operational` per app, with the
  totals on stderr. `--needs-attention` keeps the first three. With `<app>`: the
  deployment-integrity checks, the availability windows, and the browser errors
  of the last 24 hours.
- **`usage`** — the 7-day summary, the visitors, and the tracked events by name.
- **`drift`** — see below.
- **`domains`** — see [Custom domains](#custom-domains).

Times are UTC. `--json` prints one document on stdout; a table goes to stdout and
the count line to stderr.

**An empty result and a failed request are different answers.** No apps, no
builds, no errors recorded: the command says so and exits `0`. A request that
failed exits with the code its status maps to (`4`, `5`, `7`, …). Where a
command makes several independent requests (`show`, `health <app>`,
`list --builds`, `drift`), it prints what it did read, marks what it did not as
`NOT READ` (`"failed": […]` / `live_build_error` in `--json`), and then exits
non-zero. These commands report state: a failing health check, a `down` app or
a drifted app is an answer and exits `0` — read it from the output.

The health route answers **503 with its report** when a check fails; `oxyc`
prints that as a failing report, not as an outage.

### Custom domains

Serve an app on a domain the customer owns (`app.acme.com`) instead of its Oxy
subdomain. `<domain>` is a bare host name; `verify` and `remove` find the row by
that name (case-insensitive), so you never handle the domain's id.

```bash
oxyc apps domains acme/store                         # list (the default verb)
oxyc apps domains add acme/store app.acme.com        # then create the two DNS records it prints
oxyc apps domains verify acme/store app.acme.com
oxyc apps domains remove acme/store app.acme.com
```

- **`list`** — each domain's `STATUS` (`pending` or `verified`), its URL and
  when it was last checked. For every `pending` domain it prints the DNS records
  to create — a `CNAME` from the domain to the deployment's target (a note to
  ask Oxy ops when the deployment has none configured) and a `TXT` ownership
  record — plus the checker's last error, if any.
- **`add`** — registers the domain as `pending` and prints the same two records
  and the `verify` command to run next. A malformed name is `400
  invalid_domain` and a duplicate `409 domain_exists`; both exit `6` with the
  server's message.
- **`verify`** — checks DNS now. `verified` prints the URL and exits `0`. Still
  `pending` is **not** success: it prints the records again and the checker's
  reason, and exits `1` — DNS may not have propagated, so re-run it. `409
  domain_taken` (another app already serves the domain) exits `6`.
- **`remove`** — stops serving the app on that domain. A name the app does not
  have exits `5` and deletes nothing.

`--json` prints one document on stdout: `{app, app_id, domains, cname_target}`
for `list`, and `{app, app_id, domain, cname_target}` for `add` and `verify`
(`verify` still exits `1` while pending).

### `apps drift`

Compares what is live against source on **this machine**. For each app it takes
the live build's recorded repository and commit, finds a local checkout of that
repository (the ones `oxyc repos` and `oxyc path` report, or `--dir`), finds the
app's directory by the `oxy-app.json` whose `slug` and `orgSlug` match — at
`<app>/oxy-app.json` or `<app>/public/oxy-app.json`, never by folder name — and
counts the commits on the checkout's **current branch** that touch that
directory after the published commit.

| Result | Meaning |
| --- | --- |
| `in sync` | no commit on the current branch touches the app directory after the published commit |
| `N commits ahead` | that many do; they are listed |
| `unknown — <reason>` | the comparison could not be made |

`unknown` is never reported as `in sync`. Its reasons (`reason` in `--json`):

| `reason` | |
| --- | --- |
| `not_published` | nothing is live |
| `source_unrecorded` | the live build records no repository or no commit |
| `repo_not_checked_out` | the repository is not on this machine — clone it, or pass `--dir` |
| `commit_not_in_checkout` | the checkout does not have the published commit — `git fetch` there |
| `app_dir_not_found` | no tracked `oxy-app.json` declares this `slug` and `orgSlug` |
| `app_dir_ambiguous` | more than one does |
| `commit_not_on_branch` | the published commit is not an ancestor of the checked-out branch |
| `working_tree_dirty` | the app directory has uncommitted or untracked files |
| `git_failed` | `git log` or `git status` failed in the checkout, so the comparison could not be read |
| `request_failed` | the builds request failed (the command then exits non-zero) |

It runs read-only git commands and nothing else: it does not fetch, check out,
or refresh the index. The answer is therefore about the checkout as it is on
disk — a branch that is behind its remote reports fewer commits than the remote
has. With no `<app>`, every published app is compared, and `--dir` applies to
the apps whose source repository is that checkout's `origin`; with `<app>`,
`--dir` is used whatever its remote is.

An app published to two organizations from one directory (a staging org and a
production org) matches the manifest for only one of them; the other reports
`app_dir_not_found` and names the manifest it found with the other `orgSlug`.

## Checks

```bash
oxyc checks run <org-slug>/<app-slug>   # or an app UUID
oxyc checks run acme/dashboards --json --timeout 60
```

Runs every function the app declared `"check": true` (in `oxy-app.json`), one
POST per check to start its run, then polls each run to a terminal status —
`done`, `failed`, `cancelled`, or a client-side `timed_out` past `--timeout`
seconds (default 300, per check). A check **passes** when its run finishes
`done` and its answer does not parse to an object with `ok: false`.

Human mode prints one line per check to stderr as it lands (`✓ canary 4.1s` /
`✗ canary failed: <reason>`), suppressed by `--quiet`. `--json` prints one
object to stdout instead:

```json
{ "app": "acme/dashboards", "appId": "…", "checks": [{ "name": "canary", "runId": "…", "status": "done", "passed": true, "durationMs": 4123 }] }
```

**`--browser`** also loads the app's page in headless Chromium and proves it
mounted: the platform runtime's `oxy-app-ready` beacon fires, nothing on the
page throws, and `--expect-selector <css>` (optional) becomes visible. With
`--app-env` the page must also report that environment and the build it serves
(`oxyc env show`'s `build_uuid`) in `window.__OXY_APP__`. The credential and
`X-Oxy-App-Env` ride only on requests under the app's own base path
(`/customer-apps/<org>/<app>/`: the document, assets, beacon and `/fn` calls).
The rest of the deployment's API is on the same origin, and the code under test
is often a build someone else published, so its `/api/…` calls go without the
credential, as an anonymous visitor's would. The result is one more entry in `checks[]`, with `"kind": "browser"` and
the `/fn` calls the page made. Playwright is not a dependency: it is resolved
from the working directory (`playwright`, `playwright-core` or
`@playwright/test`), so run it where one is installed. The page loads as a
user, so a publish, service-account or CI token is refused before any
request. A **sandbox agent token** is the exception, in its own sandbox: the
server lets it load that sandbox's page on this path URL, with the bearer and
`X-Oxy-App-Env: dev-<handle>` and no cookie. It still needs `--app-env
dev-<handle>`. `oxy_checks_run` takes the same two options as `browser` and
`expectSelector`, and returns the same document.

```bash
oxyc checks run acme/store --app-env dev-a1 --browser --expect-selector 'main'
```

**Auth** follows the same rule as `oxyc api`'s bearer surface, with one
difference: `checks run` talks to `/api/customer-apps/**`, which is not the
`/external/api/*` surface, so a configured API key is sent explicitly as
`X-API-Key` only when **no bearer resolves** — never both. A bearer from
`oxyc login` (or `--token-env`) always wins when one is present.

**In CI, nothing is stored.** With neither a token nor an API key set, a job
holding `id-token: write` signs itself in from its GitHub OIDC token — the same
two exchanges `oxyc publish` tries, in the same order: a service account's
trust policy first, the app's registered publisher second — and drives
`/api/customer-apps/**`, the same three routes a person's credential uses. A
service account's token (`oxy_sat_…`, `oxy_ci_…`) or a publish token set in
`OXY_TOKEN` is used without minting. What differs is the app lookup: a
personal token, a legacy API key or a session can list apps to resolve
`<org>/<app>`; a machine credential may not, so it names the app by UUID or
through its own grants. `OXY_API_KEY` is read **before** OIDC here: a job that set one has said
which credential it means.

The app id comes from the credential where it can. The publisher exchange
returns it, so `<app>` must be `<org-slug>/<app-slug>` on that path (a UUID
names an app it cannot verify a publisher for). A service account's token names
the apps it may publish, so a slug resolves against its own grants — and when
it cannot, the publisher exchange is tried before asking for a UUID.

What that credential may do is deliberately small, and enforced server-side
rather than by the client. An app-scoped token is **confined to its app**: it
reaches `/customer-apps/{its own id}/…` and the upload, and answers `404` for
everything else on that surface — another app's id, the registry listing, and
the fleet-wide rollups. That is the same answer an out-of-scope App Operator
gets, so it cannot learn which apps exist. Within its app it may publish, read,
and run a function **the manifest marks `"check": true`**; a function without
the flag is `403`. The
argument for allowing the run at all is narrow: a token that may publish
arbitrary code to an app can already cause anything that app can, so running
that app's own declared checks adds nothing. It would stop being true the moment
it admitted any function.

**A check runs against the app's live build** — the server resolves
`published_build_id`, else `draft_build_id`, for the pre-flight lookup and for
the execution alike. So `checks run` after a plain `oxyc publish` verifies the
*previously promoted* code, not the draft just uploaded. (`oxyc init-ci` writes
no checks step at all today: a service account's `oxy_ci_` token is answered
`403` on these routes — the token's scope admits them for its own app, and the
routes' platform gates then refuse a caller with no platform standing.) **With
`--app-env`**
it runs against that environment's own build instead — a sandbox's own
publish, or staging's — and the report gains `environment` and each check's
`invocationId`; the publish-token and OIDC paths are unchanged and work only
against production, since every non-production operation needs a staff
credential (see [Sandboxes](#sandboxes)).

With a publish token (`oxypublish_…`), `<app>` must be the **UUID** — resolving
a slug means listing every app, which such a token may not do. A service
account's token takes either: the UUID, or a slug it can match to one of its
own app-publish grants. The publisher exchange needs no id at all (it returns
it) but does need `<org-slug>/<app-slug>`, since it is registered by slug.

**Exit codes:** `0` every check passed; `9` (`CHECK_FAILED`) at least one check
failed or timed out; `1` the app declares no checks; `5` the app was not
found; `4` no credential resolved, or the API rejected it (401/403 — an
expired legacy API key or API token reads this way, not as a missing one); `2` a bad
`--timeout`, or a slug where the credential needs a UUID; `7` (`UNAVAILABLE`)
the deployment answered the OIDC exchange without an `app_id`, meaning it
predates trusted checks.

## Sandboxes

```bash
oxyc env create <app> <name> [--from <env>] [--at <ts>] [--preset light|full] [--plane <key>=<mode> …] [--destination <db>=<mapped> …] [--fetch-host <host> …]
                                             # name is dev-<handle>, 1-12 chars a-z0-9-
oxyc env list <app>
oxyc env show <app> <name>
oxyc env update <app> <name> --plane <key>=<mode> … [--destination <db>=<mapped> …] [--clear-destination <db> …] [--fetch-host <host> …] [--replace]   # key=default clears a key
oxyc env reset <app> <name> --plane <key>[,<key>…] [--at <ts>]   # oltp, airhouse, storage, secrets, messages; --at: oltp branch, from production as of <ts>
oxyc env diff <app> <name> [--plane oltp] [--json]   # a branch sandbox's OLTP schema against production
oxyc env outbox <app> <name> [--limit <n>] [--json]    # email and notifications a `capture` environment kept
oxyc env delete <app> <name> [--yes] [--wait [seconds]]
oxyc env schedules <app> <name> [--on|--off] # staging's or a sandbox's switch; off by default

oxyc publish --app-env <name> [--env <e>]    # see "Custom apps" above
oxyc fn dev <function> [--input <json|@file|->] [--env-file <path>] [--watch] [--read-through [--env <e>]]   # local, in-process
oxyc fn call <app> <function> [--app-env <name>] [--data <json|@file|->]
oxyc fn run <app> <function> [--app-env <name>] [--as-schedule] [--data …] [--timeout <s>]
oxyc checks run <app> --app-env <name>       # see "Checks" above

oxyc invocations list <app> [--app-env] [--build] [--function] [--limit]
oxyc invocations held <app> <invocation-id>
oxyc logs <app> [--app-env] [--invocation] [--request] [--hours] [--limit]
oxyc logs <app> [--app-env <name>] --follow [--timeout <seconds>] [--json]

oxyc env secret list <app> --app-env <name>      # keys and flags, never a value
oxyc env secret set <app> <key> --app-env <name> (--value <text> | --value-env <VAR>)
oxyc env secret delete <app> <key> --app-env <name>
```

**`env create --from/--at/--preset/--plane`** branches a sandbox from production,
staging or another sandbox, optionally as of a past time, with each plane's
mode set (`--plane` repeats; it wins over `--preset`). With `--from`, the
sandbox starts on the parent's build and that build's migrations are queued for
it (a parent with no build gives a sandbox with none). You may fork only from an
environment you may open, so a sandbox agent token can fork only from a sandbox
it created, or from staging when it was minted with the staging option: `--from production`
(and `--from staging` without that option) is `400 invalid_parent` for it.
Plane keys and modes: `oltp` (`schema`, `branch`, `empty`), `airhouse`
(`write-only`, `branch`, `snapshot`), `storage` (`branch`, `empty`), `secrets` (`inherit`,
`empty`), `messages` (`invoker`, `capture`), `fetch` (`held-writes`, `allow`
with `--fetch-host`), `workspace-actions` (`held`, `refused`). `--preset full` is
`oltp=branch`, `secrets=empty`, `messages=capture` and `airhouse=branch`. **`env update`** reads
the environment's `planes_set`, merges the flags into it (`--plane a=default`
removes `a`) and PUTs the whole object; `--replace` sends exactly the flags.
Staging can be updated too; production has no planes (`409
production_has_no_planes`). **`--destination <db>=<mapped>`** makes this
environment write `<mapped>` instead of production's `<db>` over the build's
`nonProduction.destinations`, key by key; repeat it, and `env update` keeps the
ones it is not told about. **`--clear-destination <db>`** removes one (an empty
`--destination` value is a usage error). The mapped name is checked as at
publish (`400 invalid_planes`). **`--fetch-host <host>`** (repeatable) sets
the `fetch` plane to `allow` for exactly these registered provider sandboxes
(`{"mode": "allow", "hosts": [...]}`), so mutating `ctx.fetch` calls to them are
sent and listed as `(sent)` in `invocations held`; the list replaces the stored
one, a host that is not a registered sandbox is `400 invalid_planes`, and
`--plane fetch=default` turns it off. A bare `--plane fetch=allow` keeps the
stored list, and with none stored is a usage error. **`env reset`** puts the
named planes back to a clean copy of the parent's and prints each one's `done` /
`queued` (with its `run_id`): it empties `storage`, `secrets` and `messages`,
empties Airhouse's overlay, and re-cuts an `oltp=branch|empty` sandbox's own
OLTP branch from production as a queued run (`--at <ts>` cuts it as of that
time). On staging only `messages` resets (`409 plane_not_resettable` for the
others, which the team shares); a held lock is `409 environment_busy`. **`env
outbox`** lists what a `messages: capture` environment rendered and did not
send, newest first (`--limit`, default 50, at most 200; `--json` prints
`{messages, has_more}`; over MCP, `oxy_env_outbox`). A sandbox agent token sees
only messages captured since its sandbox began. A sandbox agent token may
update, reset or read the outbox of only a `dev-<handle>` sandbox it created.
`env show` prints the explicitly set planes as `set:`. **`env diff`** lists the
tables and columns a sandbox on its own OLTP branch (`oltp=branch|empty`) has
added (`+`), removed (`-`) or changed (`~`) against production's; a sandbox
without one has nothing to show, and a branch not yet cut or migrated is
`409 oltp_branch_unready` with the fix in its message.

Two plane modes name more than a keyword. **`--plane semantic=branch:<git branch>`**
pins the environment's semantic model to that branch of the app's workspace: the
server checks the branch exists (`400 invalid_planes` naming it otherwise),
compiles its head as a staging revision, and moves the pin when a later push to
the branch compiles. The environment answers `503 semantic_branch_not_ready`
until the first compile lands. `--plane semantic=default` (or `build-pin`)
returns to the build's own pin and clears the environment's. **`--plane
workspace-actions=preview`** runs an automation the app starts as a workspace
preview dry run on the environment's semantic pin (else production's revision) instead of holding or refusing it;
`--preset full` sets it.

`logs --follow` is the live tail: each line as a function prints it,
wherever the function runs, with no observability capture needed (the polled
`logs` reads ClickHouse and answers `501` without it). `--json` prints one
event per line — `open`, then `start` / `log` / `finish` per invocation,
`dropped` when lines were lost, and `done` last, on which it exits `0`. A
dropped connection is reopened with `Last-Event-ID`, and the server replays
the outcomes of what finished meanwhile, never the lines. Production without
`--app-env`; staging and sandboxes need staff with non-production reach.

A **sandbox** is a named, short-lived `dev-<handle>` environment of one custom
app: its own build pointer, storage silo, secrets, Airhouse sibling and — in
an org with an OLTP staging branch — its own copy of the app's database, with
reads hitting production and most writes either isolated or *held* (recorded,
not performed — see `invocations held`). It starts with no build, is deleted
explicitly or after 7 idle days, and an app has at most 20. Full contract,
including what is **not** isolated (`ctx.warehouse`) and the other gaps:
`internal-docs/custom-app-sandboxes.md`.

`ctx.oltp` in a sandbox runs in a schema of its own (`app_<app>__dev_<handle>`)
inside the org's staging branch. `publish --app-env` queues it: a copy of
staging's tables (rows up to a size cap), then the build's OLTP migrations.
`env show` reports it as `oltp_schema` — `status` is `seeding`, `ready`,
`failed` (with `error`) or `stale` (the branch was reset; publish again), and
`structure_only` lists the tables copied empty. Until it is `ready` a
`ctx.oltp` call in the sandbox is refused with a message that says which.

`env show` also prints one line with the environment's parent and each plane's
mode (`from staging · build=own semantic=build-pin … workspace-actions=refused`).
Those are today's behaviour for every environment, reported, not yet
configurable; `--json` carries them as `parent`, `fork_at`, `planes` and
`planes_error`.

The loop: `env create` → `publish --app-env` → `fn call` / `checks run
--app-env` → `invocations list` / `held` to read back what ran → iterate from
`publish` → `env delete --yes --wait` when done. `oxyc guide` has the same six
lines — between the two an agent adds to mint and revoke its own token — meant
to sit in an agent's context.

**`--app-env <environment>`** (`production`, `staging` or `dev-<handle>`,
validated client-side — exit `2` on a malformed name) is a *different axis*
from `--env`, which is the deployment: both can appear on the same command
(`oxyc fn call acme/store f --env dev --app-env dev-a1`). It is not a global
flag — only `publish`, `fn call`, `fn run`, `checks run`, `invocations list`
and `logs` take it. A publish-token credential is refused before any request for
`--app-env` other than `production` (exit `2`); `env`, `invocations` and
`logs` refuse one outright, for any environment, because sandbox management
is a staff console surface.

**An unattended agent drives this loop with a sandbox agent token**
(`oxy_sbx_…`), not a staff credential: `oxyc tokens create --sandbox-agent
--app <org>/<app>` mints one after a browser approval — see
[Authentication](#authentication) for what `oxyc` then refuses, and
`internal-docs/custom-app-sandboxes.md` §2 for the agent's procedure. With
it, `--app-env dev-<handle>` is required on every verb above that takes one.

**`env secret`** reads and writes one sandbox's own secrets, for any
credential that reaches the sandbox, and only a `dev-<handle>` environment —
never staging's or production's. `list` prints each key with `is_set`,
`required` and `inherits_staging`; no verb returns a value. `set` takes the
value from `--value`, or from an environment variable with `--value-env` so it
stays off the command line. A sandbox with no value of its own reads
staging's.

**`env delete`** without `--yes` asks on a terminal and exits `8` (refused)
off one, same as `oltp reset`. `--wait [seconds]` (default 120) polls until
the teardown finishes and prints `{"name","status":"deleted"}`; past the
deadline it exits `7` (retryable) rather than hanging.

**`fn call`** POSTs directly to `{target}/customer-apps/{org}/{app}/fn/{name}`
(no `/api`) — the response is Server-Sent Events, not JSON, so `oxyc` parses
the `log` / `data` / `done` / `error` frames itself. It exits `0` only when
the stream ends `done` with a 2xx status; a function that threw, or answered a
non-2xx, is a function-level failure (exit `1`, `ok: false` in the JSON), kept
distinct from a non-2xx on the POST itself (exit per the usual status mapping
— e.g. `EnvironmentHasNoBuild` before the stream starts). The
`x-oxy-invocation-id` response header becomes `invocationId` in the result.

**`fn dev`** runs a handler locally, from the app directory, with no server and
no credentials: it bundles `functions/<name>.ts` with the esbuild call `oxyc
publish` uses, runs it in-process on `createTestContext` from
`@oxy-hq/sdk/testing`, and prints the response and every host op the handler
made (`host ops: email.send`, one line per call). The context is seeded from
`oxy-app.json` — the function's capabilities and destinations — and `ctx.env`
from `--env-file` (default `.env`; never your shell). `--input` is the request
body, as `fn call --data`. `--watch` re-runs on a change under the app
directory. It exits `0` for a 2xx `Response`, `1` when the handler throws or
answers non-2xx, `2` for a function the manifest does not declare. **A local run
is not the isolate:** no timeout, memory or log limits, Node globals the isolate
lacks, and every warehouse, semantic, storage, fetch and email call is stubbed
in memory — nothing reaches a deployment. Check it where it really runs with
`fn call` against a sandbox.

`fn dev --read-through [--env <name|url>]` sends exactly `ctx.query`,
`ctx.warehouse.query` and `ctx.semantic.query` to that deployment, through
`POST /api/projects/{project}/query` and `/semantic-query` (the routes a
published app calls), authenticated as `oxyc api` is (`OXY_TOKEN` or your
`oxyc login`). The server decides what you may read. The project is `--project`
/ `OXY_PROJECT`, else looked up from `oxy-app.json` `slug` + `orgSlug`. Every
other call — writes, transactions, storage, fetch, email — stays stubbed, the
report marks each call `real` or `stubbed`, and a local run is still not the
isolate.

**`env schedules`** reads, or with `--on` / `--off` flips, the schedules
switch of staging or a sandbox (`GET`/`PUT
/api/customer-apps/{id}/environments/{name}/schedules`). Off by default: a
schedule outside production reads production data on a timer and calls third
parties with staging's credentials, so an environment fires its functions'
`schedule`s only once staff turn it on. On, it has one schedule row per
function its build schedules, kept in step by every publish to it; `--off`
removes them. Production has no switch (exit `2`) — its schedules always run.
Staff only: a sandbox agent token is refused before any request (exit `2`).
A scheduled fire, and a delivery to the environment's own webhook route
(`/api/webhooks/apps-env/{environment}/{org}/{app}/{function}`, verified with
that environment's own secret), are not a sandbox's activity.

**`fn run`** queues a function as a background run — the console's **Run
now** — and polls it to a verdict (`--timeout`, default 300 s), unlike `fn
call`, which is the function's own route. It runs as the system (the org
owner, no caller), with the function's retry policy; outside production only a
`"check": true` function runs. **`--as-schedule`** runs a function that
declares a `schedule` once, as its cron would: no input (`--data` is exit
`2`), the environment's build and policy, whether or not its switch is on —
the way to test a scheduled function in a sandbox without turning schedules
on. A function with no schedule is `409 not_scheduled` (exit `6`), production
included. `--json` prints `{function, environment, runId, status, ok, trigger,
answer, error?, invocationId}`; exit `1` when the run did not end `done`.

**`invocations held`** is what the non-production write policy did *not* do —
production's would-be effect, recorded rather than performed. It is `[]` for a
production invocation, since nothing is held there.

## Workspace previews

```bash
oxyc preview create <branch> [--wait [seconds]] [--json]
oxyc preview list [--json]
oxyc preview show <branch> [--json]
oxyc preview delete <branch> [--yes] [--json]
oxyc preview checks <branch> [--json]

oxyc preview run <branch> <kind> <ref> [--variables <json|@file|->] [--read-live-only]
                  [--window-from <iso>] [--window-to <iso>] [--resource <name>...]
                  [--wait [seconds]] [--json]
oxyc preview runs list <branch> [--json]
oxyc preview runs show <run-id> [--wait [seconds]] [--json]
```

A **preview** opens a workspace branch on the real product, against real data,
without the branch being live — staff only. `create` compiles the branch's
head into a staging revision (or reuses a ready one for that commit) and is
idempotent; `--wait [seconds]` (default 120) blocks until the compile is
`ready` or `failed` instead of returning the `compiling` item the request
itself answers with, and exits `7` (retryable) past the deadline. There is no
single-preview route: `show` fetches `list` and filters it client-side, so its
`5` (not found) is synthesized here, not a literal 404.

The loop: `preview create --wait` → `preview checks` → `preview run` →
`preview runs show --wait` → iterate → `preview delete --yes` when done.

**`<kind>`** is `procedure` or `airway_sample` — the only two `POST
/previews/runs` accepts (exit `2` client-side on anything else, naming the
read-back verb). `transform_build` and `compare` runs are queued by the
server's own Airway change check, never started here; `preview runs list`
surfaces them alongside the kinds you *can* start, and `preview runs show
<run-id> --wait` reads any of the four back — a procedure's held steps, a
transform build's or compare's outcome, or a sample's ask and result.
`--variables` / `--read-live-only` are `procedure`-only; `--window-from` /
`--window-to` / `--resource` (repeatable) are `airway_sample`-only.

Every verb needs `{workspace}` resolved — `--workspace <id>`, exactly the way
`oxyc api {workspace}/...` resolves it; there is no second workspace
resolver. A deployment with `OXY_PREVIEW_RUNS` off answers every runs route
`404 preview_runs_disabled`, surfaced the same way any other server refusal
is: the server's own `code` rides along on the thrown error, and the exit
class follows the usual HTTP-status mapping (`400`/`409` → `6`, `404` → `5`).
Full contract: `internal-docs/workspace-previews.md`.

## Development commands

```bash
oxyc validate [-f <file>] [--json]
oxyc mcp
oxyc guide
oxyc skills install | list
oxyc cache clear
oxyc exit-codes
```

**`validate`** checks workspace YAML against `json-schemas/*.json`, which are
generated from the Rust config types. No network, no token.

| File | Schema |
| --- | --- |
| `config.yml` / `config.yaml` | `config.json` |
| `*.automation.yml`, `*.procedure.yml` (`*.workflow.yml` is not a recognised kind; `oxy migrate-automations` renames it) | `workflow.json` |
| `*.agentic.yml` | `agentic.json` |
| `*.agent.test.yml` | `agent-test.json` |

Structural checks only. `oxy validate` additionally resolves `databases:` and
`llm.ref` against the loaded workspace, and wins where the two disagree.

Every **`oxy-app.json`** it finds is checked for data placement — the shape of
the data picks the store: facts and history in Airhouse (`ctx.airhouse`),
records in OLTP (`ctx.oltp`), files in `ctx.storage`, customer warehouses
read-only. These fail the run:

- `customerWarehouseWrites` that is not `{ "<database>": "<reason>" }`, or names
  a database missing from the function's `destinations`;
- `"airhouse": { "enabled": true }` in an app whose slug cannot derive a writer
  (`-` → `_`, 1–56 of `[a-z0-9_]`, starting with a letter; no `_` in the slug);
- `airhouseMigrations` without a `dir`, with one that does not exist, or with
  `migrations.dir`'s; and in its `*.sql`, `PRIMARY KEY`, `UNIQUE`,
  `CREATE INDEX`, `REFERENCES` or `FOREIGN KEY` (DuckLake has none), or a
  `CREATE`/`ALTER`/`DROP`/`INSERT`/`UPDATE`/`DELETE`/`COMMENT ON` target not
  written as `app_<writer>.<name>`.

Warnings print on stderr and do not fail: each customer-warehouse exception,
`CREATE SCHEMA` in a migration, and `ctx.secrets.set` holding state (a
`JSON.stringify` value, or a state-like key that is not a credential's). The SQL
checks are lexical; the server's parser is the authority.

Each declared **Oxy Function** is then linted — its entry and every relative
import it reaches, comments and strings masked — for the mistakes that work
locally and fail closed in production. These fail the run, each naming the
file, the line, the call and the fix:

- `capability` — a `ctx.<area>.<op>` call whose manifest capability the function
  does not declare: `ctx.secrets.set` → `secrets.write`, `ctx.email.send` →
  `email.send`, `ctx.org.*` → `org.read`, `ctx.storage.*` → `storage.read` /
  `storage.write` (`copy` needs both), `ctx.oltp.*` → `oltp.enabled`,
  `ctx.airhouse.*` → `airhouse.enabled`. The map is
  `src/publish/capabilities.ts`, held to the host's `FunctionCapabilities` by a
  Rust test.
- `isolate-global` — a value use of `Buffer`, `TextEncoder`, `TextDecoder`,
  `Blob`, `File`, `FormData`, `crypto.subtle` or `process.*`: the isolate is bare
  `deno_core` and each is a `ReferenceError` at runtime. A name the file declares
  itself (`const Buffer = …`, `import { TextEncoder } from "./polyfill"`) is not
  one; nor is a `typeof` guard or a type annotation.
- `destinations` — a `ctx.warehouse.insert` / `exec` / `upsert` or `ctx.tx` in a
  function with no `destinations`, or naming a database not in them.

The engine half — `upsert` off Postgres / DuckDB, `ctx.tx` off Postgres, a
customer-warehouse write with no `customerWarehouseWrites` reason, a write to
the org's OLTP store — needs the database list, which only the server has;
`validate` says so and `oxyc publish` runs it before the upload.

**`proxy`** forwards a local dev server's Oxy calls to a cloud target with your
login token attached. Defaults: side-effecting calls are **held**, tracking
events are **dropped**, auth endpoints reach the backend unauthenticated so
sign-in works, and the cached token never overrides a real browser session. A
production target (any host under `oxygen-hq.com`) is refused without `--yes`.
Each Oxy Function call carries a W3C trace — the SDK's, or one minted here — and
prints `↳ <status> fn <name>  request_id=…  trace_id=…`.

`--app <org>/<app> --app-env <name>` also opens the app's own page in a
non-production environment. Requests under `/customer-apps/<org>/<app>/` — the
document, its assets, the beacon and `/fn` calls — are sent with
`X-Oxy-App-Env: <name>` and **without the browser's cookie, `Origin` or
`Referer`** (the server answers `400` to an environment beside a cookie, and
its origin gate would refuse a loopback page at a cloud host; the token is the
credential). Because the browser's origin no longer reaches the server, the
proxy asks the origin gate's question itself: a request under the base must
arrive at a loopback `Host`, and an `Origin` or `Referer` it sends must be a
loopback page, and a browser request marked `Sec-Fetch-Site: cross-site` never passes, or it is refused `403 origin_not_local` before the token is
added — another site open in the same browser cannot act with it. Every
other path is forwarded as before. On start the proxy prints
`open http://127.0.0.1:<port>/customer-apps/<org>/<app>/ — dev-x of <org>/<app> on <target>`.
The name is `staging` or a `dev-<handle>` sandbox; `--app-env` alone is exit `2`,
and so is `--app-env production` (reads already hit production — leave it out).
In a **`dev-<handle>` sandbox** the app's own side-effecting calls are forwarded
without `--allow-writes`, since a sandbox's writes land in its own homes or are
held server-side; the hold applies to everything else, and to staging, as
before. Events are still dropped. For the same reason a sandbox proxy needs no
`--yes` against the product host; staging, or a sandbox with `--allow-writes`,
still does.

A **sandbox agent token** must pass `--app <org>/<app> --app-env dev-<handle>`
(exit `2` otherwise): everything else it could proxy, the server refuses.

```bash
oxyc proxy --app acme/store --app-env dev-a1    # then open the printed URL
```

**`mcp`** serves the API over stdio as four generic tools — `oxy_routes`,
`oxy_schema`, `oxy_request`, `oxy_whoami` — rather than one per endpoint, so
those schemas cost ~2 KB per turn and reach endpoints added after this
package shipped. Two named exceptions trade that minimalism for validation an
agent needs to drive a loop unsupervised: the **sandbox loop**
(`oxy_env_create`/`list`/`show`/`update`/`reset`/`outbox`/`delete`, `oxy_publish_sandbox`,
`oxy_fn_call`, `oxy_checks_run`, `oxy_invocations_list`/`held`, `oxy_logs`)
and **workspace previews** (`oxy_preview_create`/`list`/`show`/`delete`/
`checks`/`run`/`runs_list`/`run_show`). Each tool calls its CLI verb's own
request-building function, so a tool's result is the same JSON document that
verb's `--json` prints, and a failure carries the server's own error code
plus the exit-code class (`[exit 5 NOT_FOUND]`) the way `oxyc`'s exit code
would. `oxy_env_delete` / `oxy_preview_delete` need `confirm: true` (there is
no terminal to ask on inside an MCP server), and `oxy_publish_sandbox`
requires a `dev-<handle>` `appEnv` and refuses production, staging or a
promote client-side — it can never reach the live channel.

**Its credential is the token variable and nothing else.** An agent runtime
starts `oxyc mcp` with the agent's own token in `OXY_TOKEN` (or the
`--token-env` variable); with the variable unset the server exits `4` before
serving a tool, rather than running on the `oxyc login` of whoever owns the
machine. To serve on your own login, start it with `--login`.

```bash
# an agent, on its own token
claude mcp add oxyc -e OXY_TOKEN=oxy_sbx_… -- npx -y @oxy-hq/cli mcp --env dev
# you, on your login
claude mcp add oxyc -- npx -y @oxy-hq/cli mcp --login --env production
```

**A sandbox agent token (`oxy_sbx_…`) is served a different, smaller list**:
the fourteen sandbox-loop tools, `oxy_whoami` (the token's `kind`, `expires_at`,
`apps` and `minter`), and four that exist only for it — `oxy_env_secret_list`,
`oxy_env_secret_set` and `oxy_env_secret_delete`, which accept only a
`dev-<handle>` `appEnv`, and `oxy_token_revoke`, which needs `confirm: true`.
`oxy_request`, `oxy_routes`, `oxy_schema` and the preview tools are dropped,
and refused if called by name: the server answers that token `404` on all of
them, and a tool that can only fail still costs its schema every turn. A dead
token ends the server at startup with exit `4`.

**`guide`** prints a page to paste into `AGENTS.md` / `CLAUDE.md`.
**`skills install`** symlinks the bundled Oxy platform skills into
`~/.claude/skills`; it refuses to run from an `npx` cache, whose symlinks dangle
once npm reclaims it. The same skills are published to
[`oxy-hq/skills`](https://github.com/oxy-hq/skills) for every other agent —
`npx skills add oxy-hq/skills`, or the Claude Code plugin `oxy@oxy-hq`. They are
written in `skills/` here; see `internal-docs/platform-skills.md`.

## Exit codes

```
0  success
1  failure with nothing more specific to say
2  usage error — a bad flag, a missing argument, a malformed value
4  not authenticated, or the token was rejected (401/403)
5  not found (404), or an unknown customer
6  the request was malformed (4xx other than 401/403/404)
7  unavailable — 5xx, a timeout, or the network failed. Retryable.
8  refused — the operation would have destroyed or overwritten something
9  a check ran and failed or timed out (`oxyc checks run`)
```

`4` almost always means the wrong `--env`. A person logs in again. An agent on
its own token (`tokens create --agent` or `--sandbox-agent`) does not: there,
`4` means the token expired or was revoked, and the agent stops and reports.
`7` is worth retrying; `6` never is.

## Environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `OXY_TOKEN` | — | any credential, sent as a bearer; overrides the login cache and GitHub OIDC. `from-proxy`: an egress proxy adds it, so none is sent ([A token the proxy adds](#a-token-the-proxy-adds)) |
| `OXY_API_KEY` | — | a legacy API key or an API token, for the `/external/api` surface and for `checks run` |
| `OXY_SERVICE_ACCOUNT` | — | the **ID** of the service account a GitHub OIDC exchange acts as (`--service-account`); never `<org>/<name>` |
| `OXY_AGENT` | detected | the name of the agent driving `oxyc`, sent in the user agent — see below |
| `OXY_CREDENTIALS_PATH` | OS config dir | the shared `credentials.json` |
| `OXYC_ORG` | `oxy-hq` | GitHub org the customer repos live in |
| `OXYC_CUSTOMER_TOPIC` | `oxy-customer` | topic that registers a customer repo |
| `OXYC_DOSSIER_ROOT` | `~/.oxyc/dossiers` | where customer clones go |
| `OXYC_REPO_ROOTS` | scanned | where to look for our own checkouts |
| `OXYC_TEMPLATE_DIR` | shipped `template/` | use a working copy instead |
| `OXYC_SCHEMAS_DIR` | shipped `json-schemas/` | use a working copy instead |
| `OXYC_SKILLS_DIR` | shipped `skills/` | use a working copy instead |
| `OXYC_SKILLS_TARGET` | `~/.claude/skills` | where `skills install` links |
| `OXYC_CACHE_DIR` | `<cache>/oxyc` | cache root |
| `OXYC_CACHE_TTL` | `3600` | seconds the customer listing is cached |
| `OXYC_LIST_LIMIT` | `1000` | max repos listed from GitHub |
| `OXYC_SEARCH_LIMIT` | `1000` | max pull requests searched by `activity` |
| `OXYC_DIFF_LINES` | — | cap on diff lines printed by `update` |
| `OXYC_DEBUG` | — | `1` keeps the stack on an unexpected throw |
| `OXYC_QUIET` | — | `1` is `--quiet` |
| `OXYC_POLL_MS` | `3000` | how often `experiments price --wait` polls |
| `OXYC_DRY_RUN` | — | `1` makes `launch` print the command instead of running it |
| `NO_COLOR` / `FORCE_COLOR` | — | force plain / coloured output |
| `HTTPS_PROXY` / `HTTP_PROXY` | — | requests go through it. On Node 22.21+ and 24.5+, `oxyc` turns on Node's proxy support itself (`NODE_USE_ENV_PROXY`), re-running once if it has to. Older Node cannot proxy `fetch`, and the error says so |

**In a Claude Code cloud session,** the repo's SessionStart hook installs
`oxyc` and keeps an agent token across shell calls. The environment must allow
the Oxy hosts, or hold a network secret for them with `OXY_TOKEN=from-proxy`. Setup: the `oxy-custom-apps` skill, "Headless and cloud
sessions".

**An agent should set `OXY_AGENT`.** Every request `oxyc` makes to a deployment
carries the user agent `oxyc/<version>`, with `agent/<label>` after it when an
agent is driving, and `mcp` last when the call came through `oxyc mcp` —
`oxyc/0.6.0 agent/release-bot mcp`. The server records it on audit rows and in
each token's usage, and it is the one thing there that tells an agent from the
engineer running the same commands. The label is `OXY_AGENT`, held to
lowercase letters, digits, `.`, `_` and `-` and to 32 characters (anything else
becomes `-`). With `OXY_AGENT` unset, Claude Code is recognised by the
`CLAUDECODE` variable it exports and labelled `claude-code`. It identifies
honest callers in a log; it is not a credential and grants nothing.

## Output contract

**stdout carries the response body and nothing else.** Progress, warnings,
errors and hints go to stderr, and a failure never exits `0`.

Attached to a TTY you get colour, aligned tables and a searchable picker.
Piped, you get markdown tables and no colour. Raw `api` response bodies are
byte-identical in both, so `| jq` always works.

## Development

```bash
pnpm install
pnpm build          # tsdown; `prebuild` runs codegen
pnpm test           # vitest; `pretest` builds
pnpm typecheck
pnpm lint           # biome
pnpm build:binary   # standalone executables (needs bun)
```

The package ships four directories — `dist`, `json-schemas`, `skills`,
`template` — and the last three are resolved at runtime relative to
`package.json`. `scripts/ci/verify-cli-package.mjs` checks the packed tarball
on every PR.

Maintainer's notes — release process, catalog generation, OpenAPI curation, CI
gates: [`internal-docs/oxy-api-cli.md`](../../internal-docs/oxy-api-cli.md).
