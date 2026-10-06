/**
 * `oxyc publish` — build a custom app and ship the bundle to an Oxy deployment.
 *
 * From an app directory: read `oxy-app.json`, build per its `build` block (or
 * the defaults), bundle any Oxy Functions, resolve the project from the
 * target's public `build-config`, and POST the tarball. It replaced the Rust
 * `oxy publish` flag for flag; what it adds:
 *
 *   --build-only   build and bundle, stop before anything needs a credential
 *   --prebuilt     with --dir: the functions are already bundled — check, don't rebuild
 *   --json         the server's result, warnings included, on stdout
 *
 * and trusted publishing: in a GitHub Actions job granted `id-token: write`
 * with no token set, it exchanges the job's OIDC token for a credential scoped
 * to this one app.
 *
 * `--build-only` and `--prebuilt` exist for CI that keeps the credential out of
 * the job that runs package scripts: build there, publish the artifact in a job
 * that installs nothing.
 */

import { spawnSync } from "node:child_process";
import { join, resolve, sep } from "node:path";

import { APP_ENV_HEADER, requireSandboxName } from "../apps/environment.js";
import { UUID_RE } from "../apps/resolve.js";
import { fallsBackToPublisher, OidcExchangeError } from "../auth/oidc.js";
import { isMachineIdentity, isSandboxAgentToken } from "../auth/token-kind.js";
import { type Context, notAuthenticated } from "../context/resolve.js";
import { loadDotenv } from "../publish/dotenv.js";
import {
  checkEngines,
  fetchDatabaseEngines,
  functionLintFailure
} from "../publish/function-engines.js";
import {
  describeLintIssue,
  type FunctionLintIssue,
  type FunctionLintResult,
  lintAppFunctions
} from "../publish/function-lint.js";
import {
  bundleFunctions,
  enforceReservedFunctionsDir,
  requirePrebuiltFunctions,
  validateFunctionNames
} from "../publish/functions.js";
import {
  buildSteps,
  declaredFunctions,
  functionEntry,
  isValidSlug,
  loadPublishManifest,
  type PublishManifest
} from "../publish/manifest.js";
import {
  captureGitSource,
  GAP_MESSAGES,
  isRecorded,
  provenanceGaps,
  resolveBuildId,
  sanitizeRemoteUrl,
  worktreeIsDirty
} from "../publish/provenance.js";
import { compileSemanticBranch } from "../publish/semantic-branch.js";
import {
  exchangeGithubOidc,
  fetchOrgForProject,
  fetchProject,
  githubOidcAvailable,
  type PublishResult,
  uploadBundle
} from "../publish/server.js";
import { tarGzDir } from "../publish/tarball.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";

export interface PublishFlags {
  app?: string;
  buildId?: string;
  dir?: string;
  promote?: boolean;
  name?: string;
  repo?: string;
  commit?: string;
  branch?: string;
  buildOnly?: boolean;
  prebuilt?: boolean;
  json?: boolean;
  /**
   * Draft only: compile this WORKSPACE branch into a staging revision and pin
   * the build's staging preview to it. Not `branch`, the app source branch.
   */
  semanticBranch?: string;
  /** Publish past a function lint finding, printing each as a warning that names its rule. */
  allowFunctionLint?: boolean;
  /** A `dev-<handle>` sandbox to publish to instead of the draft/live channel. */
  appEnv?: string;
}

function envValue(name: string): string | undefined {
  return process.env[name]?.trim() || undefined;
}

/** `(org, app)` from a working directory shaped `…/apps/<org>/<app>[/…]`. */
export function inferOrgApp(cwd: string): { org?: string; app?: string } {
  const parts = cwd.split(sep).filter(Boolean);
  const index = parts.lastIndexOf("apps");
  if (index < 0) return {};
  return { org: parts[index + 1], app: parts[index + 2] };
}

/** How this publish will authenticate, decided before anything is built. */
type Credential = { kind: "token"; token: string } | { kind: "oidc" };

/**
 * `storedBearer`, not `bearer`: this runs before the build, and the OIDC
 * exchange must not — it spends a single-use token on a fifteen-minute
 * credential that a slow build would outlive. Only its AVAILABILITY is decided
 * here; `mintPublishToken` does the exchange, last.
 */
function resolveCredential(ctx: Context): Credential {
  const token = ctx.storedBearer();
  if (token) return { kind: "token", token };
  if (githubOidcAvailable()) return { kind: "oidc" };
  throw notAuthenticated(ctx.target(), ctx.flags);
}

/**
 * A sandbox agent token publishes into a sandbox it created and nowhere else.
 * Refused here, before the build and before any request: with no `--app-env`
 * a publish goes to the app's draft channel, and `--promote` to the live one.
 * The server refuses both too (`403 sandbox_token_refused`).
 */
function refuseOutsideOwnSandbox(token: string, flags: PublishFlags): void {
  if (!isSandboxAgentToken(token)) return;
  if (flags.appEnv === undefined) {
    throw usageError(
      flags.promote
        ? "a sandbox agent token cannot promote"
        : "a sandbox agent token cannot publish to a channel",
      "it publishes only into a sandbox it created — pass --app-env dev-<handle>, and never --promote"
    );
  }
  if (flags.semanticBranch !== undefined) {
    throw usageError(
      "--semantic-branch needs a staff credential, not a sandbox agent token",
      "compiling a workspace branch is outside what the token reaches — publish without it"
    );
  }
}

/** One manifest build step, output to stderr so stdout stays the result. */
function runBuildStep(label: string, command: string, cwd: string, basePath: string): void {
  process.stderr.write(`[${label}] $ ${command}\n`);
  const result = spawnSync("sh", ["-c", command], {
    cwd,
    stdio: ["inherit", 2, 2],
    env: { ...process.env, OXY_APP_BASE_PATH: basePath }
  });
  if (result.error) throw new CliError(`failed to start \`${command}\`: ${result.error.message}`);
  if (result.status !== 0) {
    throw new CliError(`build step \`${command}\` failed (exit ${result.status ?? "signal"})`);
  }
}

interface Identity {
  org?: string;
  app: string;
}

/** flag → env → manifest → directory, for each half. */
function resolveIdentity(ctx: Context, flags: PublishFlags, manifest?: PublishManifest): Identity {
  const inferred = inferOrgApp(ctx.cwd);
  const org = ctx.flags.org ?? envValue("OXY_ORG") ?? manifest?.orgSlug ?? inferred.org;
  const app = flags.app ?? envValue("OXY_APP") ?? manifest?.slug ?? inferred.app;
  if (!app) throw usageError("missing app", "set oxy-app.json `slug`, --app, or OXY_APP");
  if (!isValidSlug(app)) {
    throw usageError(
      `invalid app slug ${JSON.stringify(app)}`,
      "1-63 lowercase letters, digits and single hyphens — no leading, trailing or double hyphen, no underscore"
    );
  }
  return { org, app };
}

/** Build from source (unless --dir), then check and bundle `functions/`. */
function prepareBundle(
  ctx: Context,
  flags: PublishFlags,
  manifest: PublishManifest | undefined,
  identity: Identity
): string {
  const declared = declaredFunctions(manifest);
  validateFunctionNames(declared);

  let bundleDir: string;
  if (flags.dir) {
    bundleDir = resolve(ctx.cwd, flags.dir);
  } else {
    if (!identity.org) {
      throw usageError(
        "missing org: the app's base path needs it",
        "set oxy-app.json `orgSlug`, --org, OXY_ORG, or --project (a workspace determines its org), or publish a pre-built bundle with --dir"
      );
    }
    const steps = buildSteps(manifest);
    const basePath = `/customer-apps/${identity.org}/${identity.app}/`;
    runBuildStep("install", steps.install, ctx.cwd, basePath);
    runBuildStep("build", steps.command, ctx.cwd, basePath);
    bundleDir = join(ctx.cwd, steps.outDir);
  }

  const warning = enforceReservedFunctionsDir(bundleDir, declared, declared.length > 0);
  if (warning) log.warn(warning);

  if (flags.prebuilt) {
    requirePrebuiltFunctions(bundleDir, declared);
  } else {
    const entries = declared.map((name) => ({ name, entry: functionEntry(manifest, name) }));
    bundleFunctions(entries, ctx.cwd, bundleDir);
  }
  return bundleDir;
}

/**
 * The lint's findings: fail closed, or — with `--allow-function-lint` — a
 * warning each, naming the rule so a false positive can be reported by name.
 *
 * `skipped` lines are warnings either way: a function that was not read was
 * not linted, and saying so is what keeps a clean run honest.
 */
function reportFunctionLint(
  issues: FunctionLintIssue[],
  skipped: string[],
  allow: boolean | undefined
): void {
  for (const line of skipped) log.warn(`function lint: ${line}`);
  if (issues.length === 0) return;
  if (allow) {
    for (const issue of issues) log.warn(describeLintIssue(issue));
    return;
  }
  throw functionLintFailure(issues);
}

/**
 * The engine half of the lint, once the project and a credential are known.
 *
 * ADVISORY: a lookup that fails prints one warning and the publish goes on —
 * the host refuses every one of these at the first call anyway, and a publish
 * must not fail on a request the upload itself does not make.
 */
async function lintEngines(
  lint: FunctionLintResult,
  manifest: PublishManifest,
  target: string,
  project: string,
  token: string,
  allow: boolean | undefined
): Promise<void> {
  if (lint.writes.length === 0) return;
  const engines = await fetchDatabaseEngines(target, project, token);
  if ("skipped" in engines) {
    log.warn(
      "function lint: the engine check (`upsert` and `ctx.tx` dialects, customer-warehouse " +
        `writes) was skipped — could not list the project's databases: ${engines.skipped}`
    );
    return;
  }
  const issues = checkEngines(lint.writes, manifest as Record<string, unknown>, engines.databases);
  reportFunctionLint(issues, [], allow);
}

interface Provenance {
  repo?: string;
  commit?: string;
  branch?: string;
}

/** Flags win, then the checkout, then the CI variables; every gap is a warning. */
function resolveProvenance(cwd: string, flags: PublishFlags): Provenance {
  const git = captureGitSource(cwd);
  const repoSource = flags.repo ?? git.repo;
  const provenance = {
    repo: repoSource === undefined ? undefined : sanitizeRemoteUrl(repoSource),
    commit: flags.commit ?? git.commit ?? envValue("GITHUB_SHA"),
    branch: flags.branch ?? git.branch ?? envValue("GITHUB_REF_NAME")
  };
  // Only ask git about dirt when there is a recorded commit for it to contradict.
  const dirty = isRecorded(provenance.commit) ? worktreeIsDirty(cwd) : undefined;
  for (const gap of provenanceGaps(dirty, provenance.commit, provenance.repo)) {
    log.warn(GAP_MESSAGES[gap]);
  }
  return provenance;
}

/** The credential for the upload itself — the OIDC exchange happens here, last. */
async function uploadToken(
  ctx: Context,
  credential: Credential,
  identity: Identity
): Promise<string> {
  if (credential.kind === "token") return credential.token;
  return mintPublishToken(ctx, identity);
}

/**
 * A credential from the job's GitHub OIDC identity. TWO EXCHANGES, IN ORDER:
 *
 *   1. the general one (`POST /api/auth/oidc/exchange`, the deployment's own audience) — a
 *      trust policy on the service account THE WORKFLOW NAMES
 *      (`OXY_SERVICE_ACCOUNT` / `--service-account`). What every other command
 *      uses too. A workflow that names none skips this step without a request:
 *      `ctx.bearer()` raises `no_service_account` on its own.
 *   2. the app's own publisher (`POST /api/customer-apps/publish/oidc-exchange`,
 *      audience `oxy-publish`) — reached when no account is named, which is
 *      every workflow written before trust policies existed; when (1) answers
 *      404, a deployment with no general exchange; or on `no_matching_policy`,
 *      where the app may still have a publisher registered the older way.
 *
 * Any OTHER refusal from (1) is final. `missing_environment`, a self-hosted
 * runner: each says something specific is wrong with this run, and quietly
 * succeeding through the older path would hide it until the day that path is
 * retired.
 */
async function mintPublishToken(ctx: Context, identity: Identity): Promise<string> {
  log.info("exchanging the GitHub OIDC token for a credential");
  let refused: OidcExchangeError;
  try {
    return await ctx.bearer();
  } catch (cause) {
    if (!(cause instanceof OidcExchangeError) || !fallsBackToPublisher(cause)) throw cause;
    refused = cause;
  }

  const noPolicy = refused.oidcCode === "no_matching_policy";
  const unnamed = refused.oidcCode === "no_service_account";
  if (!identity.org || UUID_RE.test(identity.org)) {
    // The older exchange is keyed by slug. With no trust policy either, the
    // policy is the thing to fix — the slug only matters to the fallback.
    if (noPolicy) throw refused;
    throw usageError(
      "trusted publishing needs the org SLUG",
      "set oxy-app.json `orgSlug` or pass --org <slug> — the exchange is registered by slug"
    );
  }
  const publisher = `${identity.org}/${identity.app}'s registered publisher`;
  if (noPolicy) {
    log.info(`no trust policy of ${ctx.serviceAccount()} matches this run — trying ${publisher}`);
  } else if (unnamed) {
    log.info(`no service account is named (OXY_SERVICE_ACCOUNT) — using ${publisher}`);
  } else {
    log.info(`this deployment has no general token exchange — using ${publisher}`);
  }
  try {
    return (await exchangeGithubOidc(ctx.target(), identity.org, identity.app)).token;
  } catch (cause) {
    if (!noPolicy || !(cause instanceof CliError)) throw cause;
    // Both doors were tried and both were shut; say so, and lead with the one
    // a new registration should go through.
    throw new CliError("this workflow run is not trusted to publish", {
      code: cause.code,
      detail: [refused.detail, `publisher exchange: ${cause.message}`, cause.detail]
        .filter(Boolean)
        .join("\n"),
      hint: `${refused.hint}\n(or, the older way: ${cause.hint ?? "register the workflow as a publisher for the app"})`
    });
  }
}

/** Compile the workspace branch the draft's staging preview will read. */
async function stageSemanticBranch(
  target: string,
  token: string,
  project: string,
  branch: string
): Promise<string> {
  log.info(`compiling workspace branch ${branch} into a staging semantic revision`);
  let announced = false;
  const revision = await compileSemanticBranch(target, token, project, branch, {
    onWait: (s) => {
      if (!announced) log.info(`waiting for ${branch} @ ${s.git_sha.slice(0, 12)} to compile…`);
      announced = true;
    }
  });
  log.info(`staging preview will read semantic revision ${revision}`);
  return revision;
}

function printResult(
  target: string,
  identity: Identity,
  result: PublishResult,
  asJson: boolean
): void {
  for (const warning of result.warnings ?? []) log.warn(warning);
  if (asJson) {
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    return;
  }
  const org = result.org_slug ?? identity.org ?? "";
  const headline = result.is_new_app
    ? `Registered new app ${org}/${identity.app} (id ${result.app_id})`
    : `Published new version of ${org}/${identity.app} (id ${result.app_id})`;
  process.stdout.write(`${out.green(headline)}\n`);
  if (result.environment) {
    const where = result.environment_url
      ? result.environment_url
      : `${target}${result.url}  (no zone configured — send header ${APP_ENV_HEADER}: ${result.environment})`;
    process.stdout.write(`  build ${result.build_id} → ${result.environment} — ${where}\n`);
    return;
  }
  process.stdout.write(
    `  build ${result.build_id} → ${result.channel} channel · ${target}${result.url}\n`
  );
}

/** What one publish produced — a built-but-unsent bundle, or an uploaded one. */
export interface PublishOutcome {
  target: string;
  identity: Identity;
  /** Set at `--build-only` or with no credential: nothing was uploaded. */
  bundleDir?: string;
  /** Set once a bundle was uploaded. */
  result?: PublishResult;
}

/**
 * Build (unless `--dir`), lint, and publish — no printing. `runPublish` below
 * prints the human/`--json` report; the `oxyc mcp` sandbox-publish tool reads
 * `result` directly, so neither is a second implementation of this request.
 */
export async function publish(ctx: Context, flags: PublishFlags): Promise<PublishOutcome> {
  if (flags.prebuilt && !flags.dir) {
    throw usageError("--prebuilt needs --dir", "it names a bundle that was built elsewhere");
  }
  if (flags.prebuilt && flags.buildOnly) {
    throw usageError("--prebuilt and --build-only together do nothing", "drop one");
  }
  if (flags.semanticBranch !== undefined && flags.promote) {
    throw usageError(
      "--semantic-branch is staging-only and cannot be combined with --promote",
      "merge the branch so main compiles, then promote the build"
    );
  }
  if (flags.semanticBranch !== undefined && !flags.semanticBranch.trim()) {
    throw usageError("--semantic-branch needs a branch name");
  }
  if (flags.appEnv !== undefined) {
    // Normalized (and validated) once, here — every later read of
    // `flags.appEnv` can trust it is exactly a `dev-<handle>` name.
    flags.appEnv = requireSandboxName(flags.appEnv);
    if (flags.promote) {
      throw usageError(
        "--app-env cannot be combined with --promote",
        "a sandbox build is never promoted — publish the same tree to staging, then promote that"
      );
    }
  }

  for (const path of loadDotenv(ctx.cwd)) log.info(`loaded ${path}`);
  const manifest = loadPublishManifest(ctx.cwd);
  const identity = resolveIdentity(ctx, flags, manifest);
  const projectPin = ctx.flags.project ?? envValue("OXY_PROJECT");

  // Decided before the build, so a missing login fails in a second rather
  // than after a two-minute install.
  const credential = flags.buildOnly ? undefined : resolveCredential(ctx);
  if (flags.appEnv !== undefined && credential) {
    // Every non-production operation refuses a publish token or an OIDC
    // exchange (D22) — a sandbox publish needs a staff credential, same as
    // `env create` and `fn call` outside production.
    if (credential.kind === "oidc") {
      throw usageError(
        "--app-env needs a staff credential, not trusted publishing",
        "oxyc login, or OXY_TOKEN set to a user token"
      );
    }
    // A service account's token is a machine too — no staff standing — so the
    // sandbox route would refuse it after the build rather than before it.
    if (isMachineIdentity(credential.token)) {
      throw usageError(
        "--app-env needs a staff credential, not a publish or service-account token",
        "oxyc login, or OXY_TOKEN set to a user token"
      );
    }
  }
  if (credential?.kind === "token") refuseOutsideOwnSandbox(credential.token, flags);
  if (!identity.org && projectPin) {
    identity.org = await fetchOrgForProject(ctx.target(), projectPin);
  }

  // Before the build, like the credential: a capability the manifest lacks is
  // a one-line fix, and learning about it after a two-minute install is how
  // it used to be learned — in production, at the first call.
  const lint = manifest
    ? lintAppFunctions(ctx.cwd, manifest as Record<string, unknown>)
    : undefined;
  if (lint) reportFunctionLint(lint.issues, lint.skipped, flags.allowFunctionLint);

  const bundleDir = prepareBundle(ctx, flags, manifest, identity);
  if (flags.buildOnly || !credential) {
    if (lint && lint.writes.length > 0) {
      log.info(
        "function lint: the engine check (`upsert` and `ctx.tx` dialects, customer-warehouse " +
          "writes) needs the server — the publishing job runs it before the upload"
      );
    }
    return { target: ctx.target(), identity, bundleDir };
  }

  const target = ctx.target();
  let project = projectPin;
  if (!project) {
    if (!identity.org) {
      throw usageError(
        "missing org: needed to look up the project",
        "set oxy-app.json `orgSlug`, --org or OXY_ORG, or pass --project <uuid>"
      );
    }
    project = await fetchProject(target, identity.org, identity.app);
  }

  const buildId = resolveBuildId(flags.buildId);
  const provenance = resolveProvenance(ctx.cwd, flags);
  const tarball = tarGzDir(bundleDir);
  const channel = flags.promote ? "published" : "draft";
  const who = identity.org
    ? `${identity.org}/${identity.app}`
    : `${identity.app} → workspace ${project}`;
  log.info(`publishing ${who} (${tarball.length} bytes) → ${target} [${channel}]`);

  const token = await uploadToken(ctx, credential, identity);
  if (lint && manifest) {
    if (!isSandboxAgentToken(token)) {
      await lintEngines(lint, manifest, target, project, token, flags.allowFunctionLint);
    } else if (lint.writes.length > 0) {
      // Not attempted: `GET /api/{project}/databases` answers this token 404.
      log.info(
        "function lint: the engine check (`upsert` and `ctx.tx` dialects, customer-warehouse " +
          "writes) is skipped — a sandbox agent token cannot list the project's databases"
      );
    }
  }
  const semanticRevision = flags.semanticBranch
    ? await stageSemanticBranch(target, token, project, flags.semanticBranch.trim())
    : undefined;
  let result: PublishResult;
  try {
    result = await uploadBundle({
      target,
      token,
      tarball,
      fields: [
        ["app", identity.app],
        ["project", project],
        ["build_id", buildId],
        ["channel", channel],
        // Optional: a pre-built bundle pinned to a project lets the server infer it.
        ["org", identity.org],
        ["name", flags.name],
        ["source_repo", provenance.repo],
        ["commit_sha", provenance.commit],
        ["branch", provenance.branch],
        ["semantic_revision_id", semanticRevision],
        ["environment", flags.appEnv]
      ]
    });
  } catch (cause) {
    if (
      cause instanceof CliError &&
      cause.code === ExitCode.AUTH &&
      credential.kind === "token" &&
      // `uploadBundle` already said what a sandbox agent token should do.
      !isSandboxAgentToken(token)
    ) {
      throw new CliError(cause.message, {
        code: cause.code,
        detail: cause.detail,
        hint: `publishing needs app-admin rights on the org — \`oxyc whoami --env ${ctx.flags.env ?? "production"}\` shows yours`
      });
    }
    throw cause;
  }
  return { target, identity, result };
}

export async function runPublish(ctx: Context, flags: PublishFlags): Promise<void> {
  const outcome = await publish(ctx, flags);
  if (outcome.result) {
    printResult(outcome.target, outcome.identity, outcome.result, Boolean(flags.json));
    return;
  }
  if (flags.json) {
    process.stdout.write(`${JSON.stringify({ bundle_dir: outcome.bundleDir }, null, 2)}\n`);
  } else {
    process.stdout.write(`${out.green("built")} ${outcome.bundleDir}\n`);
  }
}
