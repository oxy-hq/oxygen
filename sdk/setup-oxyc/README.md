# setup-oxyc

A GitHub Action that installs [`oxyc`](../cli/README.md) and signs it in with
the job's own GitHub identity. **No secret is stored anywhere**: the job trades
its OIDC token for an Oxygen token that lasts fifteen minutes, acts as a
service account of your organization, and is revoked when the job ends.

```yaml
permissions: { id-token: write, contents: read }
jobs:
  deploy:
    environment: production          # the trust policy requires it
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: oxy-hq/setup-oxyc@v1
        with:
          service-account: 3f2504e0-4f89-41d3-9a0c-0305e82c3301   # acme/deployer
      - run: oxyc publish --promote   # or any oxyc command the grants allow
```

After the step, `oxyc` is on `PATH` and `OXY_TOKEN` is set for every later step
in the job. Nothing else needs configuring: `oxyc` reads `OXY_TOKEN` first.

## What it needs

- **`permissions: id-token: write`** on the job (or the workflow). Without it
  GitHub issues no OIDC token and the step fails, saying so. A job-level
  `permissions` block *replaces* the workflow-level one, so list
  `contents: read` beside it or `actions/checkout` loses its access.
- **A trust policy** on a service account, naming this repository, the workflow
  file and the job's `environment:`. An org admin registers it in the web app
  under **Organization settings → API access → Service accounts →** *the
  account* **→ Trusted access**, or `oxyc init-ci` does it from the repository.
  A run that matches no policy fails with the repository, workflow, ref and
  event it presented.
- **`environment:`** on the job, when the policy is bound to one — which it is
  by default. Without an environment anyone who can push a branch can run the
  workflow; with one, GitHub's required reviewers stand in front of it.
- **Node.js 20 or newer, and `npm`, on `PATH`.** GitHub-hosted runners have
  both. On other runners, run `actions/setup-node` first.

## Inputs

| Input | Default | |
| --- | --- | --- |
| `version` | `latest` | The `@oxy-hq/cli` version to install. Pin an exact version so a release cannot change what the job runs. |
| `service-account` | **required** | The **ID** (a UUID) of the account to act as, shown in the web app under Organization settings → API access → Service accounts → *the account*; `oxyc init-ci` writes it. The deployment matches the run against that account's trust policies and no other — it never picks an account on a run's behalf, because anyone can register a trust policy that names a repository. An ID, never `<org-slug>/<name>`: a slug is free for anyone once its org renames or is deleted, so a name can be taken over and an ID cannot. With none, or with anything that is not an ID, the step fails before installing anything. |
| `host` | `https://app.oxygen-hq.com` | Base URL of the Oxygen deployment. It must be the one the later `oxyc` commands talk to — a token is only good where it was minted. `https` only, except `localhost`. |
| `export-token` | `true` | Export the token as `OXY_TOKEN`. Set `false` to receive it only as the `token` output, and hand it to the one step that needs it. |

## Outputs

| Output | |
| --- | --- |
| `token` | The minted `oxy_ci_…` token. Masked in logs. |
| `token-id` | The token's id, as the deployment records it — what to look for in the org's API access inventory and audit trail. |
| `expires-at` | When the token expires on its own (RFC 3339). |
| `service-account` | The `<org-slug>/<name>` of the account the job is acting as — the readable name, for the log. |

To keep the token out of the job's environment and give it to one step only:

```yaml
      - id: oxy
        uses: oxy-hq/setup-oxyc@v1
        with: { service-account: 3f2504e0-4f89-41d3-9a0c-0305e82c3301, export-token: false }
      - run: oxyc publish --promote
        env:
          OXY_TOKEN: ${{ steps.oxy.outputs.token }}
```

## What it does, in order

1. Validates the inputs. They are written into an `npm install` command line,
   so anything that is not a version, a slug or a plain URL is refused.
2. Installs `@oxy-hq/cli@<version>` into a directory of its own under
   `RUNNER_TEMP`, with `--ignore-scripts`, and adds it to `PATH`. The job
   holding `id-token: write` runs no install script from the registry.
3. Asks GitHub for an OIDC token whose audience is **`oxy:<the host of
   `host`>`** — `oxy:app.oxygen-hq.com` by default — and posts it to
   `<host>/api/auth/oidc/exchange` with the ID of the `service_account` to act
   as. The audience is worked out from `host` and never asked of the
   deployment: a token is only ever for the host it is about to be sent to, so
   it is good at that deployment and no other, and a server cannot talk the
   job into minting one for somewhere else. There is no shared fallback, and
   plain `oxy` is never requested.
4. **Masks** the returned token (`::add-mask::`) before writing it anywhere,
   saves it for the post step, then exports `OXY_TOKEN` and sets the outputs.
5. **Post step, always:** `DELETE <host>/api/auth/token` revokes the token. It
   runs even when a later step failed, and it never fails the job — a revoke
   that could not be confirmed is a warning, since the token expires on its own
   within fifteen minutes.

A network error or a 5xx from the exchange is retried twice, each time with a
fresh OIDC token (one is single-use). A `429` (the exchange allows 60 requests
a minute per client address) is waited out once, for its `Retry-After` capped at
60 seconds, and retried the same way. Any other refusal is final and is not
retried.

### When the exchange refuses

| Answer | What the step says |
| --- | --- |
| `no_matching_policy` | No trust policy of the named account matches the run. Prints the repository, workflow, ref and event, and where to register a policy. |
| `service_account_required` | The request carried no account ID. The step refuses a `service-account` that is empty or not an ID itself, so this is a deployment-side answer only. |
| `missing_environment` | The policy is bound to an environment and the job declares none — or the policy names none while its organization requires one, which an org admin fixes on the policy. The deployment's own words say which. |
| `pull_request_target`, `self_hosted_runner` | The run is of a kind no policy accepts by default. |
| `wrong_audience` | The deployment answers to another address than `host`. Names that address — set `host` to it. A deployment with no public URL configured (`OXY_API_URL`) takes no GitHub sign-in at all, and the step says so. |
| `invalid_token`, `expired`, `replayed` | The GitHub token itself was not acceptable. Usually a re-run. |

### Against a deployment without trusted access

If `<host>` answers **404** to the exchange, it predates trusted access. That
is a **warning, not a failure**: `oxyc` is still installed and no `OXY_TOKEN`
is exported. `oxyc publish` and `oxyc checks run` then authenticate on their
own, through the app's registered publisher, exactly as they did before this
action existed. Any other command needs `OXY_TOKEN` set from a secret.

## Without this action

`oxyc` does the same exchange by itself whenever it runs in a job granted
`id-token: write` with no `OXY_TOKEN` set, and revokes what it minted when the
command ends. The action is worth having when a job runs several `oxyc`
commands — one exchange instead of one per command — or wants the token for
something other than `oxyc`.

```yaml
      - run: npx --yes @oxy-hq/cli@<version> publish --promote
        env:
          OXY_SERVICE_ACCOUNT: 3f2504e0-4f89-41d3-9a0c-0305e82c3301   # acme/deployer
```

## Publishing this action

**This directory lives in a private repository, so `uses: oxy-hq/setup-oxyc@v1`
does not resolve for anyone until it is mirrored to a public one.** A workflow
that references an action it cannot resolve fails at "Set up job", before any
step runs. Until the mirror exists:

- workflows in *this* repository can use `uses: ./sdk/setup-oxyc`;
- `oxyc init-ci` writes a workflow with no third-party action in it by default,
  where `oxyc` does the exchange itself. `oxyc init-ci --setup-action` writes
  one that uses this action, and warns that it will not start yet. Once the
  mirror exists, make the action `init-ci`'s default again
  (`sdk/cli/src/commands/init-ci.ts`).

To mirror it — a maintainer's step, done once and then per release:

1. Create the public repository `oxy-hq/setup-oxyc`.
2. Copy this directory's `action.yml`, `src/`, `README.md` and a `LICENSE` to
   its root. `action.yml` must sit at the root; `src/` is run as committed, so
   there is no build and no `dist/`. `test/`, `package.json` and
   `tsconfig.json` are for development and may be left out.
3. Tag the commit `v1.0.0`, and move the `v1` tag to it.

The action has **no dependencies**: it uses Node's built-in `fetch` and
`node:` modules only, so what the runner executes is exactly the reviewed
source. Keep it that way — a dependency would mean a bundle, and a bundle is a
second copy that can drift from the first.

## Development

```bash
pnpm --filter @oxy-hq/setup-oxyc test        # node --test, with a faked runner and mocked fetch
pnpm --filter @oxy-hq/setup-oxyc typecheck   # tsc --checkJs over the JSDoc annotations
```

`src/setup.mjs` is the main step and `src/cleanup.mjs` the post step;
`src/main.mjs` and `src/post.mjs` are the two-line entry points `action.yml`
names. Every effect — the network, the log, the `GITHUB_*` files, `npm` — goes
through the `Io` in `src/io.mjs`, which is what lets the tests run the real
logic against `test/fake-runner.mjs`.

The audience (`oxy:<host>`, derived from `host` exactly as the server derives
its own in `crates/auth/src/github_oidc/audience.rs`) and the refusal codes
mirror `sdk/cli/src/auth/oidc.ts`. The URL → audience cases are repeated in all
three test suites.
Change one and change the other.
