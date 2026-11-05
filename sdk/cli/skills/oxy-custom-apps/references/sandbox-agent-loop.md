# The sandbox agent loop

How an AI agent builds and tests a custom-app change on a deployment without a
person's login: its own short-lived token, its own `dev-<handle>` sandbox, and
a report its engineer can act on. What a sandbox is, and the same loop as a
person runs it, are in
[SKILL.md → Sandboxes](../SKILL.md#sandboxes--try-a-change-on-real-data-before-shipping-it).

## The whole loop at a glance

Everything up to shipping is yours; shipping is a person's.

| Step | Do | Proves |
| --- | --- | --- |
| 1. Build locally | `oxyc publish --build-only`, `oxyc fn dev <fn>` | it builds, and a function runs, before any approval |
| 2. Get a token | `oxyc tokens create --sandbox-agent --app <org>/<app>` — your engineer approves once | you act as yourself, on those apps only |
| 3. Make a sandbox | `oxyc env create … dev-x` (planes for what you test: [environment-branches.md](environment-branches.md)) | a copy of the app nobody else uses |
| 4. Publish into it | `oxyc publish --app-env dev-x` | the build the sandbox serves |
| 5. Exercise it | `fn call`, `fn run --as-schedule`, `checks run` | the functions, on production's reads |
| 6. Check the page | `checks run --browser`, `oxyc proxy --app-env dev-x` + a browser tool | what a person sees |
| 7. Read back | `invocations list` / `held`, `logs`, `env outbox`, `env diff` | what ran, held, would have been sent, and changed |
| 8. Iterate | edit, back to 4 | |
| 9. Hand back, clean up | the report below; `env delete --wait`, then `tokens revoke --current` | |
| 10. Ship | **your engineer:** `oxyc promote <app> --from dev-x`, check staging, `oxyc promote <app>` | |

## An agent's own credential: the sandbox agent token

**If you are an AI agent, you do not work on your engineer's login.** A login
carries everything the engineer can do, for 90 days. You mint a token for the
task instead (`oxy_sbx_…`): it reaches only the apps you name, your engineer
approves it once, and you end it when the task is done.

```bash
oxyc tokens create --sandbox-agent --app <org>/<app> --env <deployment>
# → a page opens in your engineer's browser: the apps, the lifetime, Approve
# → stdout, once:   export OXY_TOKEN=oxy_sbx_…
```

- **The click is your engineer's.** Tell them a page is waiting in their
  browser. The command waits five minutes for the click, so run it with a
  timeout at least that long: a shell tool that stops a command after two
  minutes kills it first. Never open or click that page with a browser tool.
- **Where no browser can reach your machine** (a cloud container, a CI job, a
  remote box), add `--no-browser`. `oxyc` prints a URL and an eight-letter
  code on stderr instead of opening a page, and waits up to ten minutes. Give
  your engineer both: they open the URL on any device where they are signed
  in, type the code the terminal shows, and approve. The token is printed
  exactly as above. The rule does not change: never open or approve that page
  yourself, with a browser tool or any other way.
- **Name who approves with `--approver <email>`** (or `OXY_APPROVER`), only
  with `--no-browser`: the engineer is told by push, email and Slack; they type
  the code from your output on the page. Use the email your engineer gave you,
  and still show them the code `oxyc` printed, since the notice does not carry
  it. Only that person can open the request: if you were given no address, omit
  the flag and hand them the URL and the code as before. Never open or click
  the page yourself.
- **Evaluate that line and work with the token.** In a shell that persists:
  `eval "$(oxyc tokens create --sandbox-agent --app <org>/<app> --env <deployment>)"`.
  In one that does not, such as an agent's shell tool, redirect it to
  `~/.config/oxy/agent-token.env` under `umask 077`, and start each command
  with `. ~/.config/oxy/agent-token.env &&`. In a Claude Code cloud session,
  the repo's SessionStart hook does that for you
  ([SKILL.md → Headless and cloud sessions](../SKILL.md#headless-and-cloud-sessions)).
  `oxyc` writes nothing to the credentials file. That file is the only place
  the token goes. Never put it in a commit or your report.
- **Make the variable the only credential.** Put `--token-env OXY_TOKEN` on
  every command after the mint, as the loop below does. With it, a command
  that lost the variable exits `4` (`OXY_TOKEN is not set`): set it and run the
  command again. Without it, `oxyc` falls back to your engineer's login cache
  on this machine, and a publish with no `--app-env` then goes to the app's
  draft channel as them.
- **Name every app the task touches** with its own `--app`, five at most.
  `<org>/<app>` is `orgSlug` and `slug` from the app's `oxy-app.json`, not the
  folder names, which can differ. When the manifest has no `orgSlug`, take the
  org slug from the app's README or from your engineer. Pass `--hours` when
  the task will outlast the default 8; the most is 168 (seven days). Apps and
  lifetime are fixed at mint, and the token cannot be extended.
- **Check it** with `oxyc whoami --env <deployment> --token-env OXY_TOKEN --json`:
  `"kind": "sandbox_agent"`, your app in `apps`, and an `expires_at` after the
  task ends. Run it first when `OXY_TOKEN` was already set before you started:
  without that `kind` it is someone else's credential, and you mint your own.
  Then `oxyc env list <org>/<app> --env <deployment> --token-env OXY_TOKEN --json`:
  exit `0` means the app and the deployment are right.
- **Set `OXY_AGENT=<your name>`** beside it, so the audit log tells your
  requests from your engineer's.
- `--env` is the deployment (`dev`, `staging`, `production`). Pass the same
  one on every command; omitted, it is `production`. Use the one your engineer
  named. When they named none, use `production` and say so when you tell them
  the approval is waiting.

For anything that is not the sandbox loop — reading data with `oxyc api`,
reproducing a bug against live data — the agent's credential is a different
token, `oxyc tokens create --agent`: see [reading-data.md](reading-data.md).

## What it reaches

One to five named apps, and only `dev-<handle>` sandboxes it created itself —
three at a time at most, counting ones still being deleted. In those it runs
the loop below and loads the app's page ([Check its page](#check-its-page)).
Everything else answers `404`: production and staging, another app, someone
else's sandbox, `/admin`, a secret's value, and the deployment's own `/api`
data routes. A publish to a channel (staging, production) or a
promote answers `403`. `oxyc` refuses those two, and `oxyc api`, `oxyc apps`
and `oxyc login-link`, before any request (exit `2`). **Shipping stays with a
person.**

## The loop

Under the token the environment must be named: `publish`, `fn call`,
`checks run`, `invocations list` and `logs` are refused without
`--app-env dev-<handle>` (exit `2`). Run `publish` from the app directory: it
reads the app from the `oxy-app.json` there. Every other command takes
`<org>/<app>`.

```bash
# 1. Create it. It starts with no build. `url` in the answer is the sandbox's link.
oxyc env create <org>/<app> dev-x --env <deployment> --token-env OXY_TOKEN --json
# 2. Build + publish into it. Success: "channel":"sandbox" and "environment":"dev-x".
oxyc publish --app-env dev-x --env <deployment> --token-env OXY_TOKEN --json
# Optional: a secret of the sandbox's own, read from a variable.
oxyc env secret set <org>/<app> KEY --app-env dev-x --value-env VAR --env <deployment> --token-env OXY_TOKEN
# 3. Call what you changed. Success: exit 0 and "ok":true. Then run the app's checks.
oxyc fn call <org>/<app> <function> --app-env dev-x --data '{}' --env <deployment> --token-env OXY_TOKEN --json
oxyc checks run <org>/<app> --app-env dev-x --env <deployment> --token-env OXY_TOKEN --json
# 3b. Changed something a person sees? Check the page (see "Check its page").
oxyc checks run <org>/<app> --app-env dev-x --browser --expect-selector '<css>' --env <deployment> --token-env OXY_TOKEN --json
# 4. Read back: what ran (a row's id is `id`), what it held instead of writing, its logs.
oxyc invocations list <org>/<app> --app-env dev-x --env <deployment> --token-env OXY_TOKEN --json
oxyc invocations held <org>/<app> <invocation-id> --env <deployment> --token-env OXY_TOKEN --json
oxyc logs <org>/<app> --app-env dev-x --env <deployment> --token-env OXY_TOKEN --json
# 5. When the task is done: delete, then revoke, in that order.
oxyc env delete <org>/<app> dev-x --yes --wait --env <deployment> --token-env OXY_TOKEN --json
oxyc tokens revoke --current --env <deployment> --token-env OXY_TOKEN
```

`dev-x` is `dev-<handle>`: 1–12 characters of `a-z`, `0-9` and single hyphens,
not starting or ending with one. Name it after the task
(`dev-<short-task-slug>`), one per agent and task, never reused across
unrelated tasks, so parallel agents do not collide. If your runtime speaks
MCP, the same loop is served as tools — see [Over MCP](#over-mcp).

## Publishing into it

Three things to know before the first publish:

- **`missing org: the app's base path needs it` (exit `2`).** `publish` takes
  the org from `--org`, `OXY_ORG`, the `orgSlug` in the directory's
  `oxy-app.json`, else the folder under an `apps/<org>/<app>/` path. An app
  whose manifest names no org, outside such a path, has none. An app that
  keeps its manifest in `public/` gets the folder name, which is wrong
  wherever the folder is not the slug. In both cases add `--org <org-slug>`,
  and `--app <slug>` when the directory has no manifest. Pass the slug, never
  a UUID: the asset base path is baked from it.
- **`pnpm install` stops with `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`
  (exit `1`).** The default install step is `pnpm install`; pnpm wants to
  rebuild `node_modules` and your shell has no terminal for it to ask on. Run
  the same command with `CI=true` in front.
- **A broken build costs an approval.** `oxyc publish --build-only` builds and
  bundles with no credential, so run it before you ask for a token (it takes
  `--org` too). `oxyc publish --dir out --prebuilt --app-env dev-x …` then
  publishes that build without building again.

## What a sandbox does with a call

Reads hit production data. A write lands in a home of the sandbox's own, or it
is *held*: recorded in `invocations held` and not performed. A held write is
the expected answer outside production. Report it. Unless the task asks for
it, do not edit `nonProduction` in the manifest to make the write land:
staging and every other sandbox share that map. To make it land in this
sandbox alone, override the destination (`env update --destination`, below). A mutating `ctx.fetch` is held
the same way unless staff set the environment's `fetch` plane to `allow` for
registered provider sandboxes (`env update --fetch-host <host>`); a call sent that way shows in `invocations held`
as `(sent)`. Do not set it yourself. A sandbox's schedules are off
unless staff turned them on, and your token cannot turn them on: to test a
scheduled function, run it once as its schedule would — `oxyc fn run
<org>/<app> <function> --app-env dev-x --as-schedule --env <deployment>
--token-env OXY_TOKEN --json` (no `--data`; `oxy_fn_run` with `asSchedule:
true` over MCP). It runs as the system, with the function's retry policy, and
answers `409 not_scheduled` (exit `6`) for a function with no `schedule`. The
`fn call` result carries the function's `logs`; `oxyc logs` shows the same
lines about 30 seconds later.
To watch a run you do not call yourself — a check on the worker fleet, a slow
function — start `oxyc logs <org>/<app> --app-env dev-x --follow --timeout 300
--env <deployment> --token-env OXY_TOKEN --json` in the background first. It
prints one JSON event per line (`open`, then `start` / `log` / `finish`),
needs no observability capture, and exits `0` on `done`. A `dropped` event
means lines were lost, not that the function failed.

## When a command fails

Branch on the exit code and the JSON fields, not on the text:

| Exit | From | It means | Do |
| --- | --- | --- | --- |
| `1` | `publish` | The build or the Oxy Function lint failed. | Fix what the error names, or see [Publishing into it](#publishing-into-it). |
| `1` | `fn call` | The function threw or answered a non-2xx: `"ok": false`. | Read `body` and `logs` in the result. It is your code. |
| `1` | `checks run` | The app declares no checks (no function has `"check": true`). | Not a failure. Verify with `fn call`, and report that the app has no checks. |
| `2` | any | The command was wrong, or the token may not do it. Nothing was sent. | Read the message. Do not retry it unchanged. |
| `4` | any | `OXY_TOKEN is not set`: the command lost the variable. Any other message: the token expired, was revoked, or your engineer lost access to the app. | Set the variable and run it again. Otherwise stop and report. Never log in. |
| `5` | any | Not found: the app, the sandbox, the function or the invocation, or one your token does not reach. From `fn call` before the first publish: the sandbox has no build. | Check the slugs and `--env` once. Do not try other spellings. From `env delete` it means already gone. |
| `6` | `env create` | The name is taken or reserved, a sandbox of that name is still tearing down, the app is at its 20-sandbox limit, or your token already holds three (`token_sandbox_limit`; one still being deleted counts). | Pick another handle, or delete one of yours with `--wait` first. Do not retry the same name. |
| `7` | any | Unavailable (network, 5xx), or a `--wait` deadline ran out. | Run it once more. If `env delete --wait` exits `7` twice, report the sandbox as not deleted. |
| `8` | `env delete` | Refused: it had no `--yes` and no terminal to ask on. | Add `--yes`. |
| `8` | anything else | A refusal. | Stop and report. Do not retry with another credential or flag. |
| `9` | `checks run` | A check failed or timed out. | Read `checks[].error`, then `invocations held` for that check's `invocationId` before assuming the function is broken: a held write can be the expected answer. |

## Rules

- **Never borrow another credential.** Do not run `oxyc login`, read the login
  cache (`credentials.json`, `oxyc token`), or use an engineer's personal
  token or API key. What the token does not reach is a person's step: ask your
  engineer. Exit `4` under the token means it expired, was revoked, or your
  engineer lost access to the app: stop and report.
- **Never put an Oxy credential into a sandbox secret.** The server refuses a
  value shaped like one (`400 credential_shaped_value`). Set a secret with
  `oxyc env secret set` (`oxy_env_secret_set`), never a hand-built request,
  and never try to read a value back.
- **Never pass `--promote`, never run `oxyc promote` or `oxyc rollback`, and
  never publish without `--app-env`.** When the sandbox build is right, say so
  and leave the ship step to your engineer: they promote that build to staging
  (`oxyc promote <app> --from dev-<handle>`), then to production. Your token is
  refused both verbs anyway. Never use `production` as `--app-env` to "check" something:
  reads already hit production.
- **A held write is information to report**, not an error to engineer around.
- **Clean up, even when a step failed.** Delete every sandbox you created,
  then revoke your token — in that order, because the delete needs the token.
  A sandbox you could not delete is removed a day after its token ends.
- **Leave the sandbox up when your engineer has to look at it**: they asked
  for its link, or the change shows on a page. Say that it is still up, and
  revoke the token once you will publish nothing more. The sandbox stays for a
  day after its token ends, and your engineer can delete it sooner with the
  `env delete` command on their own login.

## Check its page

The token loads your own sandbox's page — its HTML, assets and `/fn` calls —
when it rides as a header with `X-Oxy-App-Env: dev-x`. It never becomes a
browser cookie, so you reach the page in one of two ways, both on the token:

```bash
# 1. Assert it mounted: one more entry in checks[], "kind": "browser".
oxyc checks run <org>/<app> --app-env dev-x --browser --expect-selector '<css>' \
  --env <deployment> --token-env OXY_TOKEN --json
# 2. Drive it yourself: a local proxy that adds the token and the header.
oxyc proxy --app <org>/<app> --app-env dev-x --port 3000 \
  --env <deployment> --token-env OXY_TOKEN        # in the background; leave it running
# then, with a browser tool: http://127.0.0.1:3000/customer-apps/<org>/<app>/
```

- **The browser check** passes when the page names `dev-x` and the build `env
  show` reports, sends its ready beacon, throws nothing, and shows the
  `--expect-selector` element. It needs Playwright resolvable from the
  working directory. Over MCP it is `oxy_checks_run` with `browser: true`
  (and `expectSelector`).
- **The proxy** serves the deployment at `http://127.0.0.1:<port>`, adding the
  token and `X-Oxy-App-Env` to requests under the app's path only. Open that
  path with a browser tool (`browser_navigate`, `browser_click`,
  `browser_take_screenshot`, `browser_evaluate` to measure) and click through
  what you changed. In a sandbox it forwards writes: they land in the
  sandbox's homes or are held, as a `fn call`'s are. Stop it when you are done.
- **What it cannot show:** the page's own calls to the deployment's `/api`
  (an SDK `useQuery` or semantic query) answer `404` to your token, so a card
  fed that way renders empty or errors. `/fn` calls work. Report which cards
  you could not see with data; do not reach for another credential.

Do not log in to get a session, and do not drive the app's production URL
instead: it does not serve your build. How to drive and measure a page is in
[verifying-in-the-browser.md](verifying-in-the-browser.md).

## What to hand back

Whether the task worked or not:

- the deployment and the sandbox's name, and its link: `url` from
  `env create`, `environment_url` from `publish` (`null` means the sandbox has
  no host: say so);
- the build id(s) the publish answered;
- each function you called, with its input and whether it was `ok`; each
  check's name and `passed`, or that the app declares no checks;
- every held write (`op`, `plane`, `table`, `note`);
- the page: the browser check's result, and what you clicked through the
  proxy, with screenshots;
- what you did not verify: cards fed by the deployment's `/api` (they answer
  `404` to your token), and anything you did not open;
- the cleanup: the sandbox deleted or still up, and the token revoked.

Copy what you need out of `invocations` and `logs` into the report before you
delete the sandbox: read-back is not permanent. Never put the token in the
report.

## When you cannot get a token

Stop and ask your engineer. Do not fall back to their login in any of these
cases:

- **`oxyc` rejects `--sandbox-agent` as an unknown option (exit `2`).** The
  CLI is too old. Ask your engineer to update `@oxy-hq/cli`.
- **Exit `8`.** The deployment did not return the token you asked for: it
  predates sandbox agent tokens, or returned one for other apps. Nothing was
  printed, and what it minted was revoked.
- **Exit `7`.** Nobody approved in time (five minutes, or ten with
  `--no-browser`). Ask your engineer what the page said. Only Oxy staff
  holding `develop_apps` and `manage_apps` over the app's org can approve: if
  the page refused, report what it said and stop. If they missed it, run the
  command once more. If you named an `--approver`, say so: the request went to
  that person's phone and inbox, so they may have missed the notice rather
  than the page.
- **Exit `4`.** The approval code was rejected and nothing was minted. Run the
  command once more, with fewer `--hours` if an organization caps a token's
  lifetime. With `--no-browser`, exit `4` with `access_denied` means your
  engineer declined, or the page refused the request: the reason is printed.
  Report it and stop.
- **`--no-browser` is an unknown option (exit `2`), or the deployment has no
  device-code approval (exit `8`).** The CLI or the deployment is too old for
  a machine no browser can reach. Ask your engineer to update, or to run the
  command on a machine with a browser and hand you the token.
- **`--approver` is an unknown option (exit `2`).** The CLI predates it. Run
  without it and give your engineer the URL and the code yourself.

## Over MCP

If your runtime speaks MCP, `oxyc mcp --env <deployment>` serves the same loop
as tools — `oxy_env_create` / `oxy_env_list` / `oxy_env_show` /
`oxy_env_delete`, `oxy_env_update`, `oxy_env_reset`, `oxy_env_outbox`,
`oxy_publish_sandbox`, `oxy_fn_call`, `oxy_fn_run`, `oxy_checks_run`,
`oxy_invocations_list` / `oxy_invocations_held`, `oxy_logs`. It reads its
credential from `OXY_TOKEN` and nowhere else (a person on their own login
adds `--login`). With a sandbox agent token the list is those fourteen,
`oxy_whoami`, `oxy_env_secret_list` / `_set` / `_delete` and
`oxy_token_revoke` (`confirm: true`) — and no `oxy_request`. Prefer them to
shelling out: each calls its verb's own request-building code, so the result
is the document that verb's `--json` prints, and the choice changes nothing
on the server. There is no exit code over MCP: a failure is `isError: true`
with the class at the end of its text (`[exit 5 NOT_FOUND]`, plus `server
code: <code>` when the server sent one) — branch on that the way you would
on the exit code. `oxy_fn_call` and `oxy_checks_run` set `isError` when the
function or a check failed, with the full document still in the result.
`oxy_checks_run` takes `browser: true` and `expectSelector` for the page
check. The proxy has no tool: run `oxyc proxy` in the background.
`oxy_env_delete` needs `confirm: true`. `oxy_publish_sandbox` takes only a
`dev-<handle>` and cannot reach staging or production.

## Planes, the outbox, reset and `--from`

A sandbox's planes say how isolated it is (every plane, mode and refusal is in [environment-branches.md](environment-branches.md)). Every
command below takes `--env <deployment> --token-env OXY_TOKEN` like the rest.

- **Read what a `capture` sandbox would have sent:** `oxyc env outbox <app> dev-x
  [--limit N] [--json]` (over MCP `oxy_env_outbox`), newest first, 50 by default,
  200 at most. Each message has `kind` (`email` or `notify`), `to`, `subject` or
  `title`, `body`, `invocation_id`, `function`, `captured_at`. Your token sees
  only what was captured since *this* sandbox began, so a name you re-create
  starts empty. Assert on it after `fn call`.
- **Write to a database of its own:** `env update <app> dev-x --destination
  <db>=<mapped>` writes `<mapped>` instead of production's `<db>` in this sandbox
  only, over the manifest's `nonProduction.destinations` (`--clear-destination
  <db>` removes it; both repeat, and `env create` takes `--destination`). The
  mapped name may not be the production database or itself overridden: `400
  invalid_planes`. Prefer this to editing the manifest, which every non-production
  environment shares. Over MCP, pass `planes: {"warehouse": {"mode": "mapped",
  "destinations": {"<db>": "<mapped>"}}}`.
- **Start clean:** `env reset <app> dev-x --plane storage,secrets,messages` empties
  those planes; each answers `done`. `--plane oltp [--at <ts>]` re-cuts a sandbox's
  own OLTP branch (`oltp=branch|empty`) from production: it answers `queued` with a
  `run_id`, not `done`, so poll `env show` for `oltp_branch.status == "ready"` before
  the next call. A sandbox on the default `oltp=schema` has nothing to reset
  (`409 plane_not_resettable`): publish again to re-seed it. A second reset while
  a teardown or migration holds the sandbox is `409 environment_busy`: retry. On `staging` only `messages`
  resets (`409 plane_not_resettable` for the others: the team shares them).
- **Start on a build:** `env create dev-x --from <env>` points the new sandbox at
  the parent's build and queues that build's migrations, so you can `fn call`
  without publishing. A parent with no build gives a sandbox with none. Your token
  may fork only from a sandbox it created, or from staging if it was minted with
  the staging option: `--from production` (and `--from staging` without that
  option) answers `400 invalid_parent`, as does a name that does not exist. A person can
  fork any environment they may open.

Over MCP these are `oxy_env_outbox`, `oxy_env_update`, `oxy_env_reset` and
`oxy_env_create` (`from`, `at`, `preset`, `planes`). An outbox only fills when
the sandbox's `messages` plane is `capture`: check `oxyc env show` (`set:`)
before you read an empty one as "nothing was sent".

## A sandbox isolates the app, not the workspace

The semantic model, automations and Airway pipelines a sandbox reads are
`main`'s. To try a workspace branch, open a **workspace preview** (staff
only) — a separate loop:

```bash
oxyc preview create <branch> --wait --workspace <id> --json      # compile the branch; idempotent
oxyc preview checks <branch> --workspace <id> --json             # Airway change check of that revision
oxyc preview run <branch> procedure <file.automation.yml> --wait --workspace <id> --json  # held dry run
oxyc preview runs list <branch> --workspace <id> --json          # every run, the server's own included
oxyc preview runs show <run-id> --wait --workspace <id> --json   # one run: held steps, outcome
oxyc preview delete <branch> --yes --workspace <id> --json       # done
```

`preview run`'s kind is `procedure` or `airway_sample`; never post a
`transform_build` or `compare` run — the server queues those itself, and you
read them with `preview runs list` / `runs show`. A `--wait` verb exits `1`
when what it waited for ended `failed` or `cancelled`, `7` past its deadline.
Over MCP the same verbs are `oxy_preview_create` / `_list` / `_show` /
`_delete` (`confirm: true`) / `_checks` / `_run` / `_runs_list` / `_run_show`.
To make a sandbox read the branch's semantic model, publish to it with
`--semantic-branch <branch>`, or pin the environment itself with
`oxyc env update <app> dev-<handle> --plane semantic=branch:<branch>` (it
recompiles on a push; `503 semantic_branch_not_ready` until the first compile
lands — retry). To run an automation your bundle starts as a dry run, set
`--plane workspace-actions=preview`. `release:staging` is not available.
