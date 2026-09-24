/**
 * `oxyc init-ci` — write a GitHub Actions workflow that publishes a custom app
 * with trusted publishing: no stored secret.
 *
 * `id-token: write` is a job-level permission every step in the job can use,
 * so the workflow is TWO jobs. `build` runs the package scripts and holds no
 * credential; `publish` holds the id-token, installs nothing, and does only
 * `oxyc publish --prebuilt` on the artifact `build` produced. Do not add steps
 * to the publish job — that isolation is the security model.
 *
 * It replaced the Rust `oxy init-ci`, whose workflow called a
 * `oxy-hq/publish-action` that was never published, so no workflow it wrote
 * could run.
 */

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import type { Context } from "../context/resolve.js";
import { VERSION } from "../generated/version.js";
import { buildSteps, isValidSlug, loadPublishManifest } from "../publish/manifest.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { refusal, usageError } from "../util/errors.js";
import { parseRemoteSlug, repoRoot } from "../util/git.js";

export const WORKFLOW_PATH = ".github/workflows/oxy-publish.yml";

export interface InitCiFlags {
  app?: string;
  environment?: string;
  force?: boolean;
  promote?: boolean;
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

/**
 * Refuse any value that is not inert in both YAML and a shell word.
 *
 * Every field below is written unquoted into a `run:` line or a `path:`, and
 * one of those lines runs in the job holding `id-token: write`. The org, the
 * out dir and the app directory come from a committed manifest and a checkout
 * path, so a `$(…)` in any of them would otherwise become code in the job this
 * workflow exists to keep clean. Allowlists, not escaping: a slug, a path and
 * a URL each have a shape, and anything outside it is a mistake worth hearing.
 */
function assertWorkflowSafe(o: WorkflowOptions): void {
  const path = /^(?!\/)(?!.*(?:^|\/)\.\.(?:\/|$))[A-Za-z0-9._/-]+$/;
  const checks: Array<[string, string, boolean]> = [
    ["org", o.org, isValidSlug(o.org)],
    ["app", o.app, isValidSlug(o.app)],
    ["environment", o.environment, /^[A-Za-z0-9._-]{1,255}$/.test(o.environment)],
    ["--env", o.env, /^[A-Za-z0-9._:/-]+$/.test(o.env)],
    ["app directory", o.appDir, path.test(o.appDir)],
    ["outDir", o.outDir, path.test(o.outDir)],
    ["CLI version", o.cliVersion, /^[0-9A-Za-z.+-]+$/.test(o.cliVersion)]
  ];
  for (const [label, value, ok] of checks) {
    if (!ok) {
      throw usageError(
        `refusing to write ${label} ${JSON.stringify(value)} into a workflow`,
        "slugs, relative paths and plain URLs only — it is written unquoted into the job that holds id-token: write"
      );
    }
  }
}

/** The workflow. Pure, so the job split can be pinned by a test. */
export function workflowYaml(o: WorkflowOptions): string {
  assertWorkflowSafe(o);
  const cli = `@oxy-hq/cli@${o.cliVersion}`;
  const workdir = o.appDir === "." ? "" : `\n        working-directory: ${o.appDir}`;
  const bundlePath = o.appDir === "." ? o.outDir : `${o.appDir}/${o.outDir}`;
  const pnpm = o.pnpmFromPackageJson ? "" : "\n        with:\n          version: 10";
  const node = o.nodeVersionFile ? "node-version-file: .nvmrc" : "node-version: 20";
  const promote = o.promote ? " --promote" : "";
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
  const checksStep =
    o.hasChecks && o.promote
      ? `      # Runs every function the manifest marks \`"check": true\` against the
      # build the step above just promoted, and fails the job if any of them does
      # not pass. It mints its own short-lived credential from the same OIDC
      # token — nothing stored, and nothing carried over from the step above.
      - name: Run the app's checks
        env:
          npm_config_ignore_scripts: "true"
        run: npx --yes ${cli} checks run ${o.org}/${o.app} --env ${o.env}
`
      : "";
  return `# Generated by \`oxyc init-ci\`. Trusted publishing — no stored secret.
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
    # Attach required reviewers to this environment in Settings → Environments.
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
      - name: Publish
        env:
          # The CLI's own dependencies run no install scripts in this job.
          npm_config_ignore_scripts: "true"
        run: npx --yes ${cli} publish --prebuilt${promote} --dir ${o.outDir} --env ${o.env} --org ${o.org} --app ${o.app}${workdir}
${checksStep}`;
}

/** `gh api users/<owner> --jq .id`, when gh is here and answers. */
function ownerId(owner: string): string | undefined {
  try {
    const result = spawnSync("gh", ["api", `users/${owner}`, "--jq", ".id"], { encoding: "utf8" });
    return result.status === 0 ? result.stdout.trim() || undefined : undefined;
  } catch {
    return undefined;
  }
}

export function runInitCi(ctx: Context, flags: InitCiFlags): void {
  const { org, app } = resolveApp(ctx.cwd, flags.app);
  const environment = flags.environment?.trim() || "oxy-publish";
  const root = repoRoot(ctx.cwd) ?? ctx.cwd;
  const path = join(root, WORKFLOW_PATH);
  if (existsSync(path) && !flags.force) {
    throw refusal(`${WORKFLOW_PATH} already exists`, { hint: "pass --force to overwrite it" });
  }

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
      env: ctx.flags.env ?? "production",
      appDir,
      outDir: buildSteps(manifest).outDir,
      hasChecks,
      promote: flags.promote === true,
      pnpmFromPackageJson: packageJsonPinsPnpm(root),
      nodeVersionFile: existsSync(join(root, ".nvmrc")),
      cliVersion: VERSION
    })
  );

  const remote = spawnSync("git", ["remote", "get-url", "origin"], { cwd: root, encoding: "utf8" });
  const slug = remote.status === 0 ? parseRemoteSlug(remote.stdout) : undefined;
  const [owner = "<owner>", repo = "<repo>"] = slug?.split("/") ?? [];
  const id = slug ? ownerId(owner) : undefined;

  process.stdout.write(`${out.green("wrote")} ${path}\n\n`);
  // A manifest with a check and a workflow without the step is the one
  // surprising outcome here, so say it out loud rather than leaving the author
  // to notice a step that was never written.
  if (hasChecks && !flags.promote) {
    log.info(
      "the manifest declares a check, but this workflow publishes a draft — a check runs the LIVE build, so it would verify the previously promoted one. Re-run with --promote to publish live and verify what this workflow ships."
    );
  }
  log.info("next: register this workflow as a publisher for the app, then gate the environment");
  process.stdout.write(
    `  oxyc api /api/customer-apps/<app-id>/publishers --env ${ctx.flags.env ?? "production"} -X POST \\\n` +
      `    -f repo_owner=${owner} -F repo_owner_id=${id ?? `<numeric id: gh api users/${owner} --jq .id>`} \\\n` +
      `    -f repo_name=${repo} -f workflow_ref=${WORKFLOW_PATH} -f environment=${environment}\n\n` +
      `  <app-id> is ${org}/${app}'s id on its admin page. Then add required reviewers to the\n` +
      `  '${environment}' environment in Settings → Environments.\n`
  );
}
