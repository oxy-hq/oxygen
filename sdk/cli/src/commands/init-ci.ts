/**
 * `oxyc init-ci` — write a GitHub Actions workflow that publishes a custom app
 * with no stored secret, and register the trust that lets it.
 *
 * `id-token: write` is a job-level permission every step in the job can use,
 * so the workflow is TWO jobs. `build` runs the package scripts and holds no
 * credential; `publish` holds the id-token, installs nothing but the pinned
 * CLI, and does only `oxyc publish --prebuilt` on the artifact `build`
 * produced. Do not add steps to the publish job — that isolation is the
 * security model.
 *
 * The publish job gets its credential from `oxyc` itself: the job's OIDC
 * token, exchanged for a fifteen-minute token acting as a service account and
 * revoked when the command ends. What makes the deployment honour that is a
 * TRUST POLICY naming this repository, this workflow file and the job's
 * `environment:` — so this command creates one when it can
 * (`init-ci-register.ts`) and prints the exact steps when it cannot.
 *
 * THE WORKFLOW USES NO THIRD-PARTY ACTION BY DEFAULT. It replaced the Rust
 * `oxy init-ci`, whose workflow called a `oxy-hq/publish-action` that was never
 * published, so no workflow it wrote could run — a job that references an
 * action GitHub cannot resolve fails at "Set up job", before any step. The
 * `setup-oxyc` action (`sdk/setup-oxyc`) is in the same position until it is
 * mirrored to a public `oxy-hq/setup-oxyc`, so it is opt-in: `--setup-action`
 * writes `uses: oxy-hq/setup-oxyc@v1`, which does the exchange once for the
 * whole job. Flip the default back only once that repository exists.
 */

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import type { Context } from "../context/resolve.js";
import { VERSION } from "../generated/version.js";
import { buildSteps, isValidSlug, loadPublishManifest } from "../publish/manifest.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { refusal, usageError } from "../util/errors.js";
import { repoRoot, slugFromRemote } from "../util/git.js";
import {
  DEFAULT_SERVICE_ACCOUNT,
  isValidServiceAccountName,
  lookUpAccountId,
  type Registration,
  type RegistrationPlan,
  registerTrustPolicy
} from "./init-ci-register.js";

export const WORKFLOW_PATH = ".github/workflows/oxy-publish.yml";

/** The action the publish job gets its credential from. */
export const SETUP_ACTION = "oxy-hq/setup-oxyc@v1";

export interface InitCiFlags {
  app?: string;
  environment?: string;
  force?: boolean;
  promote?: boolean;
  /** `--no-register`: write the workflow, touch nothing on the deployment. */
  register?: boolean;
  /** `--setup-action`: get the credential from the `setup-oxyc` action. Off by default. */
  setupAction?: boolean;
}

export interface WorkflowOptions {
  org: string;
  app: string;
  environment: string;
  /** `--env` the publish step targets. */
  env: string;
  /** App directory relative to the repo root, `.` when they are the same. */
  appDir: string;
  outDir: string;
  /** Omit `version:` when package.json pins pnpm, or pnpm/action-setup refuses. */
  pnpmFromPackageJson: boolean;
  /** Whether the manifest declares any `"check": true` function to verify with. */
  hasChecks: boolean;
  /** `--promote`: publish to the live channel, and verify it with the checks. */
  promote: boolean;
  nodeVersionFile: boolean;
  cliVersion: string;
  /** `<org>/<name>` — the account the publish job acts as, for a person to read. */
  serviceAccount: string;
  /**
   * The account's id — what the workflow actually NAMES it by, since that is
   * all the deployment's exchange takes. Absent when it could not be looked
   * up: a `<service-account-id>` placeholder is written then, never the name.
   */
  serviceAccountId?: string;
  /** Use the `setup-oxyc` action, rather than `npx` and oxyc's own exchange. */
  setupAction: boolean;
  /** The deployment's base URL, for the action. Unused without it. */
  host: string;
  /** The app's id, when known: a service account's token cannot resolve a slug. */
  appId?: string;
}

/** `org/app` from `--app`, else the manifest in the working directory. */
function resolveApp(cwd: string, flag: string | undefined): { org: string; app: string } {
  const manifest = loadPublishManifest(cwd);
  const [org, app] = flag ? flag.split("/") : [manifest?.orgSlug, manifest?.slug];
  if (!org || !app || (flag !== undefined && flag.split("/").length !== 2)) {
    throw usageError(
      "which app does this workflow publish?",
      "pass --app <org-slug>/<app-slug>, or run it from a directory whose oxy-app.json names both"
    );
  }
  return { org, app };
}

/**
 * `--service-account <org>/<name>` (or a bare name), else `<app's org>/deployer`.
 *
 * The NAME, here and only here: `init-ci` is run by a person, who knows the
 * account by what it is called. It looks the id up as that person and writes
 * the id into the workflow — which is what every other command, and the
 * deployment, take.
 */
function resolveAccount(
  named: string | undefined,
  appOrg: string
): { org: string; name: string; named: boolean } {
  if (!named) return { org: appOrg, name: DEFAULT_SERVICE_ACCOUNT, named: false };
  if (UUID.test(named)) {
    throw usageError(
      "init-ci takes the service account's name, and looks its ID up itself",
      `pass --service-account <org-slug>/<name> (or drop it for ${appOrg}/${DEFAULT_SERVICE_ACCOUNT}); unset OXY_SERVICE_ACCOUNT if it carries an ID here. The ID is what it writes into the workflow.`
    );
  }
  const parts = named.split("/");
  const [org, name] = parts.length === 1 ? [appOrg, parts[0]] : parts;
  if (parts.length > 2 || !org || !name) {
    throw usageError(
      `--service-account ${JSON.stringify(named)} is not <org-slug>/<name>`,
      "e.g. --service-account acme/deployer"
    );
  }
  // The server's own rule, checked before a workflow is written or an account
  // is looked up: a name it would refuse can never be the one that exists.
  if (!isValidServiceAccountName(name)) {
    throw usageError(
      `${JSON.stringify(name)} is not a valid service account name`,
      "lowercase letters and digits in words joined by single hyphens, starting with a letter, 2–40 characters — e.g. deployer, release-bot"
    );
  }
  // A service account's grants are in its own org only: the server refuses an
  // `app_publish` grant on another org's app, so such an account could never
  // publish this one.
  if (org !== appOrg) {
    throw usageError(
      `${org}/${name} cannot publish ${appOrg}'s apps: a service account is granted only its own organization`,
      `name an account of ${appOrg}, e.g. --service-account ${appOrg}/${name}`
    );
  }
  return { org, name, named: true };
}

function packageJsonPinsPnpm(dir: string): boolean {
  try {
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")) as {
      packageManager?: string;
    };
    return typeof pkg.packageManager === "string" && pkg.packageManager.startsWith("pnpm@");
  } catch {
    return false;
  }
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * Refuse any value that is not inert in both YAML and a shell word.
 *
 * Every field below is written unquoted into a `run:` line, a `with:` or a
 * `path:`, and most of those sit in the job holding `id-token: write`. The
 * org, the out dir and the app directory come from a committed manifest and a
 * checkout path, so a `$(…)` in any of them would otherwise become code in the
 * job this workflow exists to keep clean. Allowlists, not escaping: a slug, a
 * path and a URL each have a shape, and anything outside it is a mistake worth
 * hearing.
 */
function assertWorkflowSafe(o: WorkflowOptions): void {
  const path = /^(?!\/)(?!.*(?:^|\/)\.\.(?:\/|$))[A-Za-z0-9._/-]+$/;
  const [accountOrg = "", accountName = "", ...extra] = o.serviceAccount.split("/");
  const checks: Array<[string, string, boolean]> = [
    ["org", o.org, isValidSlug(o.org)],
    ["app", o.app, isValidSlug(o.app)],
    ["environment", o.environment, /^[A-Za-z0-9._-]{1,255}$/.test(o.environment)],
    ["--env", o.env, /^[A-Za-z0-9._:/-]+$/.test(o.env)],
    ["app directory", o.appDir, path.test(o.appDir)],
    ["outDir", o.outDir, path.test(o.outDir)],
    ["CLI version", o.cliVersion, /^[0-9A-Za-z.+-]+$/.test(o.cliVersion)],
    [
      "service account",
      o.serviceAccount,
      extra.length === 0 && isValidSlug(accountOrg) && isValidServiceAccountName(accountName)
    ],
    [
      "service account id",
      o.serviceAccountId ?? "",
      o.serviceAccountId === undefined || UUID.test(o.serviceAccountId)
    ],
    ["app id", o.appId ?? "", o.appId === undefined || UUID.test(o.appId)]
  ];
  if (o.setupAction) {
    checks.push(["deployment URL", o.host, /^https?:\/\/[A-Za-z0-9._:/-]+$/.test(o.host)]);
  }
  for (const [label, value, ok] of checks) {
    if (!ok) {
      throw usageError(
        `refusing to write ${label} ${JSON.stringify(value)} into a workflow`,
        "slugs, relative paths and plain URLs only — it is written unquoted into the job that holds id-token: write"
      );
    }
  }
}

/** What a workflow carries where the account's id could not be looked up. */
export const SERVICE_ACCOUNT_ID_PLACEHOLDER = "<service-account-id>";

/**
 * The value a workflow names its service account by, with the readable name
 * as a trailing YAML comment: `3f25…3301   # acme/deployer`.
 *
 * ALWAYS THE ID, never the name. `<org>/<name>` can be re-pointed — a slug is
 * free for anyone once its org renames or is deleted — so the deployment
 * takes only the id, and a workflow carrying the name would be refused on its
 * first run. With no id in hand this writes a placeholder a person must
 * replace, which fails the same way and says why.
 */
function accountValue(o: WorkflowOptions): string {
  return o.serviceAccountId
    ? `${o.serviceAccountId}   # ${o.serviceAccount}`
    : `${SERVICE_ACCOUNT_ID_PLACEHOLDER}   # REPLACE with the ID of ${o.serviceAccount}`;
}

/**
 * The publish job's steps after `setup-node`: how it gets a credential, the
 * publish, and the checks.
 *
 * WITH THE ACTION, one step installs the pinned CLI, exchanges, exports
 * `OXY_TOKEN` and revokes it afterwards, and every later step is a plain
 * `oxyc …`. WITHOUT IT, each step runs `npx` and `oxyc` exchanges for itself —
 * once per step, since nothing carries a token between them.
 */
function credentialedSteps(o: WorkflowOptions, workdir: string): string {
  const promote = o.promote ? " --promote" : "";
  const publishArgs = `publish --prebuilt${promote} --dir ${o.outDir} --env ${o.env} --org ${o.org} --app ${o.app}`;
  // Two conditions, and the second is the one that is easy to get wrong.
  //
  // A check runs against the app's LIVE build — `trigger_function_job` and the
  // executor both resolve `published_build_id.or(draft_build_id)`, so a run
  // started right after a DRAFT publish executes the previously promoted code
  // and reports on the wrong artifact. Worse, the first workflow to add a check
  // would fail: the live build predates the flag, so `checks run` finds none and
  // exits 1. So the step is emitted only alongside `--promote`, where the build
  // just published IS the one the check runs.
  //
  // And only when there is something to run: `oxyc checks run` on an app that
  // declares no check is an error, not a no-op.
  const withChecks = o.hasChecks && o.promote;
  // By id when it is known. A service account's token may not list apps, so a
  // slug is resolved from the token's own grants — which works, but the id
  // needs nothing resolved at all.
  const checksArgs = `checks run ${o.appId ?? `${o.org}/${o.app}`} --env ${o.env}`;
  const checksComment = `      # Runs every function the manifest marks \`"check": true\` against the
      # build the step above just promoted, and fails the job if any of them does
      # not pass.`;

  if (o.setupAction) {
    const checks = withChecks
      ? `${checksComment}
      - name: Run the app's checks
        run: oxyc ${checksArgs}
`
      : "";
    return `      # Installs the pinned oxyc without running any install script, exchanges
      # this job's OIDC token for a fifteen-minute token acting as the service
      # account, exports it as OXY_TOKEN, and revokes it when the job ends.
      - uses: ${SETUP_ACTION}
        with:
          version: ${o.cliVersion}
          service-account: ${accountValue(o)}
          host: ${o.host}
      - name: Publish
        run: oxyc ${publishArgs}${workdir}
${checks}`;
  }

  const cli = `@oxy-hq/cli@${o.cliVersion}`;
  const env = `        env:
          # The CLI's own dependencies run no install scripts in this job.
          npm_config_ignore_scripts: "true"
          # Which service account this job's OIDC token is exchanged for, by its
          # ID: a name could be taken over by another organization, an ID cannot.
          OXY_SERVICE_ACCOUNT: ${accountValue(o)}`;
  const checks = withChecks
    ? `${checksComment} It mints its own short-lived credential from the same
      # OIDC identity — nothing stored, and nothing carried over from the step above.
      - name: Run the app's checks
${env}
        run: npx --yes ${cli} ${checksArgs}
`
    : "";
  return `      # No stored secret and no token step: oxyc exchanges this job's OIDC
      # token itself, and revokes what it minted when the command ends.
      - name: Publish
${env}
        run: npx --yes ${cli} ${publishArgs}${workdir}
${checks}`;
}

/** The workflow. Pure, so the job split can be pinned by a test. */
export function workflowYaml(o: WorkflowOptions): string {
  assertWorkflowSafe(o);
  const cli = `@oxy-hq/cli@${o.cliVersion}`;
  const workdir = o.appDir === "." ? "" : `\n        working-directory: ${o.appDir}`;
  const bundlePath = o.appDir === "." ? o.outDir : `${o.appDir}/${o.outDir}`;
  const pnpm = o.pnpmFromPackageJson ? "" : "\n        with:\n          version: 10";
  const node = o.nodeVersionFile ? "node-version-file: .nvmrc" : "node-version: 20";
  return `# Generated by \`oxyc init-ci\`. No stored secret: the publish job trades its
# GitHub OIDC token for a short-lived Oxy token, as the service account
# ${o.serviceAccount}.
#
# \`id-token: write\` is job-level and visible to every step, so the publish job
# is isolated: it checks out the commit, downloads what \`build\` produced, and
# publishes it. It runs no package script. Keep it that way — add nothing to it,
# and pin these actions by SHA.
name: Publish ${o.org}/${o.app} to Oxy

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  build:
    runs-on: ubuntu-latest
    # No id-token here: this job runs every dependency's install scripts.
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - uses: pnpm/action-setup@v4${pnpm}
      - uses: actions/setup-node@v4
        with:
          ${node}
      # Runs oxy-app.json's build and bundles its Oxy Functions into the output
      # directory — everything a publish does except the upload.
      - name: Build the bundle
        run: pnpm dlx ${cli} publish --build-only --org ${o.org} --app ${o.app}${workdir}
      - uses: actions/upload-artifact@v4
        with:
          name: oxy-bundle
          path: ${bundlePath}
          # Vite writes \`.vite/manifest.json\`; the default drops hidden files.
          include-hidden-files: true
          if-no-files-found: error

  publish:
    needs: build
    runs-on: ubuntu-latest
    # The trust policy names this environment, so a job without it gets no
    # token. Attach required reviewers to it in Settings → Environments.
    environment: ${o.environment}
    permissions:
      id-token: write
      contents: read
    steps:
      # For oxy-app.json and the commit the build is recorded against.
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - uses: actions/download-artifact@v4
        with:
          name: oxy-bundle
          path: ${bundlePath}
      - uses: actions/setup-node@v4
        with:
          ${node}
${credentialedSteps(o, workdir)}`;
}

/** What `init-ci` is registering, for the two messages below. */
interface Subject {
  org: string;
  app: string;
  account: string;
  repository: string;
  environment: string;
  env: string;
  /** The deployment's base URL, for a link to the page. Absent when unresolved. */
  host?: string;
}

/**
 * Organization settings → API access, as a link. `?settings=<section>` is the
 * web app's deep link into its settings dialog; a deployment without the
 * section drops the parameter and lands on Home.
 */
export const API_ACCESS_SETTINGS_SECTION = "organization.api_access";

function registeredMessage(s: Subject, r: Extract<Registration, { kind: "registered" }>): string {
  const made = [
    r.createdAccount ? `created service account ${s.account}` : `service account ${s.account}`,
    r.createdPolicy ? "added its trust policy" : "its trust policy was already there"
  ].join(", ");
  return (
    `${out.green("registered")} ${made}\n` +
    `  ${s.account} may publish ${s.org}/${s.app} from ${s.repository}\n` +
    `  (${WORKFLOW_PATH}, environment '${s.environment}') — and nothing else.\n`
  );
}

/**
 * The steps, exactly, with every id this run managed to learn filled in.
 *
 * The web app first: both rows need a browser session to create, so the API
 * calls only work from a login that IS one — which a fresh `oxyc login` no
 * longer is. They are printed anyway, for the operator who has one.
 */
export function manualSteps(s: Subject, r: Extract<Registration, { kind: "manual" }>): string {
  const [owner = "<owner>", repo = "<repo>"] = s.repository.split("/");
  const [, accountName = DEFAULT_SERVICE_ACCOUNT] = s.account.split("/");
  const orgId = r.orgId ?? "<org-id>";
  const appId = r.appId ?? "<app-id>";
  const accountId = r.accountId ?? "<service-account-id>";
  const lookups = [
    r.orgId
      ? ""
      : `    <org-id>   oxyc api /api/orgs --env ${s.env} -q '.[] | select(.slug=="${s.org}") | .id'`,
    r.appId
      ? ""
      : `    <app-id>   oxyc api /api/orgs/${orgId}/apps --env ${s.env} -q '.[] | select(.slug=="${s.app}") | .id'`,
    r.accountId ? "" : "    <service-account-id>   the `id` the first call returns"
  ].filter(Boolean);
  const page = s.host
    ? `\n    ${s.host.replace(/\/+$/, "")}/?settings=${API_ACCESS_SETTINGS_SECTION}`
    : "";

  return `  In the web app, as an org admin — Organization settings → API access → Service accounts:${page}
    1. create the service account \`${accountName}\`, role member, unless it exists
    2. open it and, under Trusted access, add a trust policy:
         repository    ${s.repository}
         workflow      ${WORKFLOW_PATH}
         environment   ${s.environment}
         grant         publish ${s.org}/${s.app} — and nothing else

  Or over the API, from a browser-session login (a token cannot create these):
    oxyc api /api/orgs/${orgId}/service-accounts --env ${s.env} -X POST \\
      -f name=${accountName} -f org_role=member
    oxyc api /api/orgs/${orgId}/service-accounts/${accountId}/trust-policies --env ${s.env} -X POST \\
      -f repository=${s.repository} -f workflow_path=${WORKFLOW_PATH} -f environment=${s.environment} \\
      -F 'grants=[{"kind":"app_publish","app_id":"${appId}"}]'
${lookups.length > 0 ? `${lookups.join("\n")}\n` : ""}
  A deployment without trust policies takes the older registration instead —
  the workflow as a publisher of the app:
    oxyc api /api/customer-apps/${appId}/publishers --env ${s.env} -X POST \\
      -f repo_owner=${owner} -F repo_owner_id=<numeric id: gh api users/${owner} --jq .id> \\
      -f repo_name=${repo} -f workflow_ref=${WORKFLOW_PATH} -f environment=${s.environment}
`;
}

/** The deployment's URL when it resolves. A link is a nicety, never a reason to fail. */
function targetIfResolvable(ctx: Context): string | undefined {
  try {
    return ctx.target();
  } catch {
    return undefined;
  }
}

export async function runInitCi(ctx: Context, flags: InitCiFlags): Promise<void> {
  const { org, app } = resolveApp(ctx.cwd, flags.app);
  const environment = flags.environment?.trim() || "oxy-publish";
  const root = repoRoot(ctx.cwd) ?? ctx.cwd;
  const path = join(root, WORKFLOW_PATH);
  if (existsSync(path) && !flags.force) {
    throw refusal(`${WORKFLOW_PATH} already exists`, { hint: "pass --force to overwrite it" });
  }

  const account = resolveAccount(ctx.serviceAccount(), org);
  const serviceAccount = `${account.org}/${account.name}`;
  // Opt-in: the action is not published yet, and a workflow naming it cannot start.
  const setupAction = flags.setupAction === true;
  // Resolved only for the action, which needs a URL where `oxyc` takes a name.
  const host = setupAction ? ctx.target() : "";
  // `--target` overrides `--env`, and the workflow carries only an `--env`: so
  // a target given here is written as the env (a URL is one), or the job would
  // exchange its token at one deployment and publish to another.
  const env = ctx.flags.target ? ctx.target() : (ctx.flags.env ?? "production");
  const repository = slugFromRemote(root);

  const plan: RegistrationPlan = {
    org,
    app,
    accountOrg: account.org,
    accountName: account.name,
    accountNamed: account.named,
    repository,
    workflowPath: WORKFLOW_PATH,
    environment
  };
  // Before the file is written, because the app id it learns goes INTO the
  // file; after the overwrite refusal, so a refused run creates nothing.
  const registration: Registration =
    flags.register === false
      ? { kind: "manual", reason: "--no-register was passed" }
      : await registerTrustPolicy(ctx, plan);

  // The workflow names the account by id. Registration usually learned it;
  // when it did not run, or stopped short, ask for just that — read-only.
  const serviceAccountId = await lookUpAccountId(ctx, plan, registration);

  const appDir = relative(root, ctx.cwd).split(sep).join("/") || ".";
  const manifest = loadPublishManifest(ctx.cwd);
  const hasChecks = Object.values(manifest?.functions ?? {}).some((fn) => fn?.check === true);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(
    path,
    workflowYaml({
      org,
      app,
      environment,
      env,
      appDir,
      outDir: buildSteps(manifest).outDir,
      hasChecks,
      promote: flags.promote === true,
      pnpmFromPackageJson: packageJsonPinsPnpm(root),
      nodeVersionFile: existsSync(join(root, ".nvmrc")),
      cliVersion: VERSION,
      serviceAccount,
      serviceAccountId,
      setupAction,
      host,
      appId: registration.appId
    })
  );

  process.stdout.write(`${out.green("wrote")} ${path}\n\n`);
  if (!serviceAccountId) {
    // Said before anything else: the file as written cannot sign in.
    log.warn(
      `the workflow names its service account as ${SERVICE_ACCOUNT_ID_PLACEHOLDER}: the ID of ${serviceAccount} could not be looked up from here`
    );
    log.hint(
      `replace the placeholder with the account's ID — Organization settings → API access → Service accounts → ${account.name} — or re-run \`oxyc init-ci --force\` once it exists and you are logged in. The name itself is never accepted: an ID cannot be taken over by another organization.`
    );
  }
  // A manifest with a check and a workflow without the step is the one
  // surprising outcome here, so say it out loud rather than leaving the author
  // to notice a step that was never written.
  if (hasChecks && !flags.promote) {
    log.info(
      "the manifest declares a check, but this workflow publishes a draft — a check runs the LIVE build, so it would verify the previously promoted one. Re-run with --promote to publish live and verify what this workflow ships."
    );
  }

  const subject: Subject = {
    org,
    app,
    account: serviceAccount,
    repository: repository ?? "<owner>/<repo>",
    environment,
    env,
    host: host || targetIfResolvable(ctx)
  };
  if (registration.kind === "registered") {
    process.stdout.write(registeredMessage(subject, registration));
  } else {
    log.info("next: register a trust policy, so the deployment trusts this workflow");
    log.hint(`not done for you: ${registration.reason}`);
    process.stdout.write(`\n${manualSteps(subject, registration)}`);
  }
  process.stdout.write(
    `\n  Then add required reviewers to the '${environment}' environment in Settings → Environments.\n`
  );
  if (setupAction) {
    log.warn(
      `the workflow uses ${SETUP_ACTION}, which is not published yet: until it is, the job fails at "Set up job"`
    );
    log.hint(
      "re-run with --force and without --setup-action to have oxyc exchange the token itself"
    );
  }
}
