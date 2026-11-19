/**
 * The workflow `oxyc init-ci` writes. Its value is the job split — the job
 * holding `id-token: write` runs no package script — so that is what is pinned,
 * structurally, on the parsed YAML rather than by grepping text.
 */

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parse } from "yaml";

import { CliError, ExitCode } from "../util/errors.js";
import {
  manualSteps,
  NO_CHECKS_STEP,
  SERVICE_ACCOUNT_ID_PLACEHOLDER,
  SETUP_ACTION,
  WORKFLOW_PATH,
  type WorkflowOptions,
  workflowYaml
} from "./init-ci.js";
import {
  DEFAULT_NODE_VERSION,
  DEFAULT_PNPM_MAJOR,
  packageJsonPinsPnpm,
  pnpmWarnings
} from "./init-ci-pnpm.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const APP_ID = "0b0e5a10-1111-4222-8333-944455556666";

interface Step {
  name?: string;
  uses?: string;
  run?: string;
  with?: Record<string, unknown>;
  env?: Record<string, string>;
  "working-directory"?: string;
}
interface Job {
  permissions?: Record<string, string>;
  environment?: string;
  steps: Step[];
}

/** The id of `acme/deployer`: what the workflow names the account by. */
const ACCOUNT_ID = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";

const OPTIONS: WorkflowOptions = {
  org: "acme",
  app: "sales",
  environment: "oxy-publish",
  env: "production",
  appDir: ".",
  outDir: "out",
  pnpmFromPackageJson: false,
  nodeVersionFile: false,
  cliVersion: "9.9.9",
  hasChecks: false,
  promote: false,
  serviceAccount: "acme/deployer",
  serviceAccountId: ACCOUNT_ID,
  setupAction: true,
  host: "https://app.oxygen-hq.com"
};

/** The same workflow with no third-party action: what `init-ci` writes by default. */
const INLINE: WorkflowOptions = { ...OPTIONS, setupAction: false, host: "" };

function jobs(options: WorkflowOptions): { build: Job; publish: Job } {
  return (parse(workflowYaml(options)) as { jobs: { build: Job; publish: Job } }).jobs;
}

describe("workflowYaml", () => {
  /**
   * NO CHECKS STEP, IN ANY SHAPE. The job's credential is a service account's
   * `oxy_ci_` token, and the server answers it 403 on the check routes: they
   * sit behind platform gates a service account never passes
   * (`trusted_publish.rs` pins it). A generated step would exit 4 on every
   * promoted publish of every app that declares a check.
   */
  it("never writes a checks step — the job's token could not run one", () => {
    for (const base of [OPTIONS, INLINE]) {
      for (const o of [
        base,
        { ...base, hasChecks: true },
        { ...base, promote: true },
        { ...base, hasChecks: true, promote: true }
      ]) {
        const { publish } = jobs(o);
        expect(publish.steps.map((s) => s.name ?? s.uses)).not.toContain("Run the app's checks");
        expect(JSON.stringify(publish)).not.toContain("checks run");
        // The publish is the last step, and nothing is stored.
        expect(publish.steps.at(-1)?.name ?? publish.steps.at(-1)?.run).toMatch(/Publish|publish/);
        expect(JSON.stringify(publish)).not.toContain("secrets.");
      }
    }
  });

  it("leaves one comment line where the step would be, and only there", () => {
    for (const base of [OPTIONS, INLINE]) {
      // Where it would have been written: a promoted publish of an app with a check.
      const yaml = workflowYaml({ ...base, hasChecks: true, promote: true });
      const lines = yaml.split("\n").filter((line) => line.includes("oxyc checks run"));
      expect(lines).toEqual([NO_CHECKS_STEP.trimEnd()]);
      expect(lines[0]).toMatch(/^ {6}# No `oxyc checks run` step: /);
      expect(lines[0]).toContain("until ci tokens may run checks");
      // After the publish step, and still a workflow GitHub can parse.
      expect(yaml.indexOf("oxyc checks run")).toBeGreaterThan(yaml.indexOf("- name: Publish"));
      expect(jobs({ ...base, hasChecks: true, promote: true }).publish.steps.at(-1)?.name).toBe(
        "Publish"
      );

      // Nowhere else: no check declared, or a draft publish.
      for (const o of [base, { ...base, hasChecks: true }, { ...base, promote: true }]) {
        expect(workflowYaml(o)).not.toContain("checks run");
      }
    }
  });

  it("publishes a draft unless --promote", () => {
    const publishStep = (o: WorkflowOptions) =>
      jobs(o).publish.steps.find((s) => s.name === "Publish")?.run ?? "";
    expect(publishStep(OPTIONS)).not.toContain("--promote");
    expect(publishStep({ ...OPTIONS, promote: true })).toContain("--promote");
  });

  it("gives the id-token to the publish job only, and binds it to an environment", () => {
    for (const base of [OPTIONS, INLINE]) {
      const { build, publish } = jobs(base);
      expect(build.permissions?.["id-token"]).toBeUndefined();
      expect(publish.permissions).toEqual({ "id-token": "write", contents: "read" });
      // The trust policy names the environment; without it there is no token.
      expect(publish.environment).toBe("oxy-publish");
    }
  });

  /** The isolation IS the security model: nothing in that job may run a package script. */
  it("with --setup-action, keeps the publish job to checkout, download, node, the action and one publish", () => {
    const { publish } = jobs(OPTIONS);
    expect(publish.steps.map((s) => s.uses ?? "run")).toEqual([
      "actions/checkout@v4",
      "actions/download-artifact@v4",
      "actions/setup-node@v4",
      SETUP_ACTION,
      "run"
    ]);
    // The action is told exactly which CLI, which account and which deployment.
    expect(publish.steps[3]?.with).toEqual({
      version: "9.9.9",
      "service-account": ACCOUNT_ID,
      host: "https://app.oxygen-hq.com"
    });
    const run = publish.steps[4];
    expect(run?.run).toBe(
      "oxyc publish --prebuilt --dir out --env production --org acme --app sales"
    );
    expect(JSON.stringify(publish)).not.toMatch(/pnpm|npm install|build-only/);
  });

  it("by default, uses no third-party action and lets oxyc exchange", () => {
    const { publish } = jobs(INLINE);
    expect(publish.steps.map((s) => s.uses ?? "run")).toEqual([
      "actions/checkout@v4",
      "actions/download-artifact@v4",
      "actions/setup-node@v4",
      "run"
    ]);
    const run = publish.steps[3];
    expect(run?.run).toBe(
      "npx --yes @oxy-hq/cli@9.9.9 publish --prebuilt --dir out --env production --org acme --app sales"
    );
    expect(run?.env).toEqual({
      npm_config_ignore_scripts: "true",
      OXY_SERVICE_ACCOUNT: ACCOUNT_ID
    });
    expect(JSON.stringify(publish)).not.toMatch(/pnpm|install|build-only|setup-oxyc/);
  });

  /**
   * THE ID IS THE VALUE; THE NAME IS A COMMENT. The deployment takes only the
   * id — `acme/deployer` could be taken over by whoever gets the slug `acme`
   * next — so the name never appears where YAML would read it as the value.
   */
  it("names the account by id, with the readable name as a trailing comment", () => {
    for (const options of [OPTIONS, INLINE]) {
      const yaml = workflowYaml(options);
      const key = options.setupAction ? "service-account" : "OXY_SERVICE_ACCOUNT";
      expect(yaml).toContain(`${key}: ${ACCOUNT_ID}   # acme/deployer`);
      expect(yaml).not.toMatch(new RegExp(`${key}: acme/`));
    }
  });

  it("writes a placeholder — never the name — when the id is not known", () => {
    for (const options of [OPTIONS, INLINE]) {
      const unknown = { ...options, serviceAccountId: undefined };
      const yaml = workflowYaml(unknown);
      const key = options.setupAction ? "service-account" : "OXY_SERVICE_ACCOUNT";
      expect(yaml).toContain(
        `${key}: ${SERVICE_ACCOUNT_ID_PLACEHOLDER}   # REPLACE with the ID of acme/deployer`
      );
      expect(yaml).not.toMatch(new RegExp(`${key}: acme/`));
      // What a YAML parser hands the job: the placeholder, which the
      // deployment refuses with `service_account_required`.
      const step = jobs(unknown).publish.steps.at(options.setupAction ? 3 : -1);
      const value = options.setupAction ? step?.with?.["service-account"] : step?.env?.[key];
      expect(value).toBe("<service-account-id>");
    }
  });

  it("builds and bundles in the job without the credential, pinned to this CLI", () => {
    const { build } = jobs(OPTIONS);
    const runs = build.steps.filter((s) => s.run).map((s) => s.run);
    expect(runs).toEqual([
      "pnpm dlx @oxy-hq/cli@9.9.9 publish --build-only --org acme --app sales"
    ]);
    const upload = build.steps.find((s) => s.uses === "actions/upload-artifact@v4");
    expect(upload?.with).toMatchObject({ path: "out", "include-hidden-files": true });
    // The build job runs every dependency's install scripts: no action that
    // mints a token belongs in it.
    expect(JSON.stringify(build)).not.toContain("setup-oxyc");
  });

  it("runs from the app directory when it is not the repo root", () => {
    const { build, publish } = jobs({ ...OPTIONS, appDir: "apps/acme/sales", outDir: "dist" });
    expect(build.steps.find((s) => s.run)?.["working-directory"]).toBe("apps/acme/sales");
    expect(publish.steps.at(-1)?.["working-directory"]).toBe("apps/acme/sales");
    const download = publish.steps.find((s) => s.uses === "actions/download-artifact@v4");
    expect(download?.with?.path).toBe("apps/acme/sales/dist");
  });

  /**
   * Every value lands unquoted in a `run:` line, a `with:` or a `path:`, and
   * most of those lines run in the job holding `id-token: write`. An `orgSlug`
   * of `acme$(curl …|sh)` in a committed manifest must not become a workflow.
   */
  it("refuses a value that would run as shell in the generated workflow", () => {
    for (const bad of [
      { org: "acme$(id)" },
      { app: "sales;id" },
      { environment: "oxy publish" },
      { env: "dev && id" },
      { outDir: "out`id`" },
      { outDir: "../elsewhere" },
      { appDir: "apps/a b" },
      { cliVersion: "1.0.0 || id" },
      { serviceAccount: "acme/deployer; id" },
      { serviceAccount: "deployer" },
      // A slug, but not a name the server would accept for a service account.
      { serviceAccount: "acme/1bot" },
      // The id is written unquoted too, and only an id is one.
      { serviceAccountId: "acme/deployer" },
      { serviceAccountId: `${ACCOUNT_ID}; id` },
      { host: "https://app.oxygen-hq.com/$(id)" },
      { host: "javascript:alert(1)" }
    ]) {
      expect(() => workflowYaml({ ...OPTIONS, ...bad }), JSON.stringify(bad)).toThrow(CliError);
    }
  });

  it("still accepts a deployment URL as the env and a nested out dir", () => {
    const { publish } = jobs({ ...OPTIONS, env: "https://acme.oxygen-hq.com", outDir: "dist/spa" });
    expect(publish.steps.at(-1)?.run).toContain("--env https://acme.oxygen-hq.com");
  });

  const setupNode = (job: Job) => job.steps.find((s) => s.uses === "actions/setup-node@v4");

  /**
   * v4 does not support pnpm 11. With nothing in the repo to defer to, the
   * workflow says pnpm 11 and a Node that runs it (pnpm 11 needs >= 22.13).
   */
  it("installs pnpm 11 with action-setup v6, on Node 26, when the repo pins neither", () => {
    for (const base of [OPTIONS, INLINE]) {
      const { build, publish } = jobs(base);
      const pnpm = build.steps.filter((s) => s.uses?.startsWith("pnpm/action-setup@"));
      expect(pnpm.map((s) => s.uses)).toEqual(["pnpm/action-setup@v6"]);
      expect(pnpm[0]?.with).toEqual({ version: 11 });
      // pnpm before setup-node, as in the template: setup-node may shell out to it.
      expect(build.steps.indexOf(pnpm[0] as Step)).toBeLessThan(
        build.steps.indexOf(setupNode(build) as Step)
      );
      for (const job of [build, publish]) {
        expect(setupNode(job)?.with).toEqual({ "node-version": 26 });
      }
    }
  });

  /**
   * pnpm/action-setup v6 installs package.json's pin and THROWS "Multiple
   * versions of pnpm specified" on a `version:` that differs from it — `11`
   * against `pnpm@11.22.0` included — so a pinned repo gets no `version:` at
   * all. The repo's `.nvmrc` likewise wins over the default Node, in both jobs.
   */
  it("defers to package.json's pnpm and the repo's .nvmrc when present", () => {
    const { build, publish } = jobs({
      ...OPTIONS,
      pnpmFromPackageJson: true,
      nodeVersionFile: true
    });
    const pnpm = build.steps.find((s) => s.uses === "pnpm/action-setup@v6");
    expect(pnpm).toBeDefined();
    expect(pnpm?.with).toBeUndefined();
    for (const job of [build, publish]) {
      expect(setupNode(job)?.with).toEqual({ "node-version-file": ".nvmrc" });
    }
    // The two rules are independent: a pin with no .nvmrc, and the reverse.
    expect(setupNode(jobs({ ...OPTIONS, pnpmFromPackageJson: true }).build)?.with).toEqual({
      "node-version": 26
    });
    const unpinned = jobs({ ...OPTIONS, nodeVersionFile: true }).build;
    expect(unpinned.steps.find((s) => s.uses === "pnpm/action-setup@v6")?.with).toEqual({
      version: 11
    });
  });

  /**
   * The defaults are the workspace template's own pins, so a repo oxyc wrote
   * CI for and a repo it scaffolded install the same pnpm on the same Node.
   */
  it("defaults to the pnpm and Node the workspace template pins", () => {
    const template = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "template");
    const pkg = JSON.parse(readFileSync(join(template, "package.json"), "utf8")) as {
      packageManager: string;
      engines: { node: string };
    };
    expect(pkg.packageManager.split("@")[1]?.split(".")[0]).toBe(DEFAULT_PNPM_MAJOR);
    expect(readFileSync(join(template, ".nvmrc"), "utf8").trim()).toBe(DEFAULT_NODE_VERSION);
    // pnpm 11.22.0 declares `engines.node: >=22.13`; the template says the same.
    expect(pkg.engines.node).toBe(">=22.13");
    // setup-node resolves a bare major to its newest release, and every 22.x
    // from 22.13 on runs pnpm 11; Node 20 does not.
    expect(Number(DEFAULT_NODE_VERSION)).toBeGreaterThanOrEqual(22);
  });
});

describe("packageJsonPinsPnpm", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "oxyc-pins-pnpm-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  const pins = (pkg: unknown) => {
    writeFileSync(join(dir, "package.json"), JSON.stringify(pkg));
    return packageJsonPinsPnpm(dir);
  };

  /** Both are places pnpm/action-setup v6 reads a version from. */
  it("counts a packageManager or a devEngines.packageManager naming pnpm", () => {
    expect(pins({ packageManager: "pnpm@11.22.0+sha512.abc" })).toBe(true);
    expect(pins({ packageManager: "pnpm@10.33.0" })).toBe(true);
    expect(pins({ devEngines: { packageManager: { name: "pnpm", version: "11.22.0" } } })).toBe(
      true
    );
  });

  it("does not count another package manager, a nameless pin, or no package.json", () => {
    expect(pins({ packageManager: "yarn@4.5.0" })).toBe(false);
    expect(pins({ devEngines: { packageManager: { name: "pnpm" } } })).toBe(false);
    expect(pins({ engines: { pnpm: ">=11" } })).toBe(false);
    expect(pins({ devEngines: null })).toBe(false);
    writeFileSync(join(dir, "package.json"), "{ not json");
    expect(packageJsonPinsPnpm(dir)).toBe(false);
    rmSync(join(dir, "package.json"));
    expect(packageJsonPinsPnpm(dir)).toBe(false);
  });
});

/**
 * init-ci runs in repos it did not scaffold. One still on pnpm 10 conventions
 * — no pin, `onlyBuiltDependencies` — gets a workflow running pnpm 11, which
 * ignores that key and fails the install: said when the file is written.
 */
describe("pnpmWarnings", () => {
  let dir: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "oxyc-pnpm-warnings-"));
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  const repoWith = (pkg: unknown, workspace?: string) => {
    writeFileSync(join(dir, "package.json"), JSON.stringify(pkg));
    if (workspace !== undefined) writeFileSync(join(dir, "pnpm-workspace.yaml"), workspace);
    return pnpmWarnings(dir).map((w) => w.warning);
  };
  const OLD_WAY = "onlyBuiltDependencies:\n  - esbuild\n";
  const NEW_WAY = "allowBuilds:\n  esbuild: true\n";

  it("says nothing for a repo shaped like the template", () => {
    expect(repoWith({ packageManager: "pnpm@11.22.0+sha512.abc" }, NEW_WAY)).toEqual([]);
    expect(repoWith({ packageManager: "pnpm@11.22.0" })).toEqual([]);
  });

  it("names the pnpm an unpinned repo gets, and how to pin its own", () => {
    repoWith({ private: true }, NEW_WAY);
    const [only, ...rest] = pnpmWarnings(dir);
    expect(rest).toEqual([]);
    expect(only?.warning).toBe("package.json pins no pnpm, so the workflow installs pnpm 11");
    expect(only?.hint).toContain("corepack use pnpm@<version>");
  });

  it("warns about onlyBuiltDependencies wherever the build runs pnpm 11", () => {
    const ignored = /onlyBuiltDependencies`, which pnpm 11 ignores/;
    // Unpinned (pnpm 11 by default), pinned to 11, or a pin too loose to rule 11 out.
    expect(repoWith({}, OLD_WAY)).toEqual([expect.any(String), expect.stringMatching(ignored)]);
    for (const pin of [
      { packageManager: "pnpm@11.22.0" },
      { devEngines: { packageManager: { name: "pnpm", version: "^11" } } },
      { devEngines: { packageManager: { name: "pnpm", version: "latest" } } }
    ]) {
      expect(repoWith(pin, OLD_WAY), JSON.stringify(pin)).toEqual([expect.stringMatching(ignored)]);
    }
  });

  it("leaves a repo pinned to pnpm 10, or already on allowBuilds, alone", () => {
    expect(repoWith({ packageManager: "pnpm@10.33.0" }, OLD_WAY)).toEqual([]);
    // Both keys: allowBuilds is the one pnpm 11 reads, so the repo has moved.
    expect(repoWith({ packageManager: "pnpm@11.22.0" }, `${OLD_WAY}${NEW_WAY}`)).toEqual([]);
    // An unreadable workspace file is not evidence of the old key.
    expect(repoWith({ packageManager: "pnpm@11.22.0" }, "{ not: [yaml")).toEqual([]);
  });
});

describe("manualSteps", () => {
  const SUBJECT = {
    org: "acme",
    app: "sales",
    account: "acme/deployer",
    repository: "acme-co/acme-apps",
    environment: "oxy-publish",
    env: "dev"
  };

  it("names the exact policy, and how to find each id it could not learn", () => {
    const steps = manualSteps(SUBJECT, { kind: "manual", reason: "not logged in" });
    expect(steps).toContain("repository    acme-co/acme-apps");
    expect(steps).toContain(`workflow      ${WORKFLOW_PATH}`);
    expect(steps).toContain("environment   oxy-publish");
    expect(steps).toContain("publish acme/sales — and nothing else");
    expect(steps).toContain("/api/orgs/<org-id>/service-accounts --env dev -X POST");
    expect(steps).toContain('"kind":"app_publish","app_id":"<app-id>"');
    // Each placeholder comes with the command that fills it.
    expect(steps).toContain('select(.slug=="acme")');
    expect(steps).toContain('select(.slug=="sales")');
  });

  it("says where in the web app, and links the page when the deployment is known", () => {
    const where = "Settings → Organization → API tokens → Service accounts";
    const unlinked = manualSteps(SUBJECT, { kind: "manual", reason: "not logged in" });
    expect(unlinked).toContain(where);
    expect(unlinked).toContain("under Trusted access, add a trust policy");
    expect(unlinked).not.toContain("?settings=");

    const linked = manualSteps(
      { ...SUBJECT, host: "https://app.oxygen-hq.com/" },
      { kind: "manual", reason: "not logged in" }
    );
    expect(linked).toContain(where);
    expect(linked).toContain("https://app.oxygen-hq.com/?settings=organization.api_access");
  });

  it("fills in the ids a partial registration did learn", () => {
    const steps = manualSteps(SUBJECT, {
      kind: "manual",
      reason: "creating the service account needs a browser session",
      orgId: "org-1",
      appId: APP_ID
    });
    expect(steps).toContain("/api/orgs/org-1/service-accounts --env dev -X POST");
    expect(steps).toContain(`"app_id":"${APP_ID}"`);
    expect(steps).not.toContain("<org-id>");
    expect(steps).not.toContain("<app-id>");
    // Still unknown, so still explained.
    expect(steps).toContain("<service-account-id>");
  });
});

describe("oxyc init-ci, through the binary", () => {
  let repo: string;
  beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "oxyc-init-ci-"));
    spawnSync("git", ["init", "-q"], { cwd: repo });
    spawnSync("git", ["remote", "add", "origin", "git@github.com:acme-co/acme-apps.git"], {
      cwd: repo
    });
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  // `HOME` is the scratch repo and the credentials path does not exist, so
  // there is no login to register with: these cases never touch the network.
  const run = (cwd: string, args: string[]) =>
    spawnSync(process.execPath, [BIN, "init-ci", ...args], {
      cwd,
      encoding: "utf8",
      env: {
        PATH: process.env.PATH ?? "",
        HOME: repo,
        NO_COLOR: "1",
        OXY_TOKEN: "",
        OXY_CREDENTIALS_PATH: join(repo, "no-such-credentials.json")
      }
    });

  /** GitHub reads workflows from the repo root, wherever the command was run. */
  it("writes the workflow at the repo root from an app directory, and prints the registration", () => {
    const appDir = join(repo, "apps", "acme", "sales");
    mkdirSync(appDir, { recursive: true });
    writeFileSync(
      join(appDir, "oxy-app.json"),
      JSON.stringify({ schemaVersion: 2, slug: "sales", orgSlug: "acme" })
    );

    const result = run(appDir, ["--env", "dev"]);
    expect(result.status, result.stderr).toBe(0);
    const written = readFileSync(join(repo, WORKFLOW_PATH), "utf8");
    expect(written).toContain("working-directory: apps/acme/sales");
    expect(written).toContain("--env dev");
    // No third-party action by default: `oxy-hq/setup-oxyc` is not published, and a
    // workflow naming an action GitHub cannot resolve fails before its first step.
    expect(written).not.toContain("setup-oxyc");
    expect(written).not.toMatch(/^\s*host:/m);
    // Nobody is logged in, so the account's id cannot be looked up: the
    // workflow carries a placeholder to replace — never the name — and the
    // command says so.
    expect(written).toContain(
      "OXY_SERVICE_ACCOUNT: <service-account-id>   # REPLACE with the ID of acme/deployer"
    );
    expect(written).not.toContain("OXY_SERVICE_ACCOUNT: acme/");
    expect(result.stderr).toContain("the ID of acme/deployer could not be looked up");
    expect(written).toContain("environment: oxy-publish");
    expect(result.stderr).not.toContain("--setup-action");

    // Nobody is logged in, so the policy is not created — and the steps say
    // exactly what to register, and why it was not done.
    expect(result.stderr).toContain("not done for you: not logged in");
    expect(result.stdout).toContain("repository    acme-co/acme-apps");
    expect(result.stdout).toContain("-f repository=acme-co/acme-apps");
    expect(result.stdout).toContain(`-f workflow_path=${WORKFLOW_PATH}`);
    // The older registration, for a deployment without trust policies.
    expect(result.stdout).toContain("-f repo_owner=acme-co");
    expect(result.stdout).toContain("-f repo_name=acme-apps");
    expect(result.stdout).toContain(`-f workflow_ref=${WORKFLOW_PATH}`);
  });

  /**
   * A member may not read the org's accounts, so a `--force` re-run could not
   * look the ID up again: the one the workflow already names for this account
   * is kept, and the placeholder never goes back over it.
   */
  it("keeps the ID the workflow already names for the account when it can't look it up", () => {
    const appDir = join(repo, "apps", "acme", "sales");
    mkdirSync(appDir, { recursive: true });
    writeFileSync(
      join(appDir, "oxy-app.json"),
      JSON.stringify({ schemaVersion: 2, slug: "sales", orgSlug: "acme" })
    );
    const workflow = join(repo, WORKFLOW_PATH);
    mkdirSync(dirname(workflow), { recursive: true });
    const id = "3f25a6c1-7b0e-4d2a-9c11-0e5d2b7a3301";

    writeFileSync(workflow, `env:\n  OXY_SERVICE_ACCOUNT: ${id}   # acme/deployer\n`);
    const kept = run(appDir, ["--env", "dev", "--force"]);
    expect(kept.status, kept.stderr).toBe(0);
    const written = readFileSync(workflow, "utf8");
    expect(written).toContain(`OXY_SERVICE_ACCOUNT: ${id}   # acme/deployer`);
    expect(written).not.toContain(SERVICE_ACCOUNT_ID_PLACEHOLDER);
    expect(kept.stderr).toContain(`kept the ID the workflow already named for acme/deployer`);
    expect(kept.stderr).not.toContain("replace the placeholder");

    // Another account's ID is not this one's: the placeholder, as before.
    writeFileSync(workflow, `env:\n  OXY_SERVICE_ACCOUNT: ${id}   # acme/release-bot\n`);
    const other = run(appDir, ["--env", "dev", "--force"]);
    expect(other.status, other.stderr).toBe(0);
    expect(readFileSync(workflow, "utf8")).toContain(
      "OXY_SERVICE_ACCOUNT: <service-account-id>   # REPLACE with the ID of acme/deployer"
    );
  });

  /** What the repo root pins is read from the repo root, not from where it ran. */
  it("reads the pnpm pin and .nvmrc from the repo root", () => {
    const appDir = join(repo, "apps", "acme", "sales");
    mkdirSync(appDir, { recursive: true });
    writeFileSync(
      join(appDir, "oxy-app.json"),
      JSON.stringify({ schemaVersion: 2, slug: "sales", orgSlug: "acme" })
    );
    const build = () =>
      (parse(readFileSync(join(repo, WORKFLOW_PATH), "utf8")) as { jobs: { build: Job } }).jobs
        .build;
    const pnpmStep = (job: Job) => job.steps.find((s) => s.uses === "pnpm/action-setup@v6");
    const nodeStep = (job: Job) => job.steps.find((s) => s.uses === "actions/setup-node@v4");

    // Nothing pinned, and build scripts approved the pnpm 10 way: pnpm 11 and
    // the default Node — and the command says what that will do to the build.
    writeFileSync(join(repo, "pnpm-workspace.yaml"), "onlyBuiltDependencies:\n  - esbuild\n");
    const unpinned = run(appDir, ["--no-register"]);
    expect(unpinned.status, unpinned.stderr).toBe(0);
    expect(pnpmStep(build())?.with).toEqual({ version: 11 });
    expect(nodeStep(build())?.with).toEqual({ "node-version": 26 });
    expect(unpinned.stderr).toContain(
      "package.json pins no pnpm, so the workflow installs pnpm 11"
    );
    expect(unpinned.stderr).toContain("which pnpm 11 ignores");

    // Pinned at the root, as the template pins it: no `version:`, and .nvmrc.
    writeFileSync(
      join(repo, "package.json"),
      JSON.stringify({ private: true, packageManager: "pnpm@11.22.0+sha512.abc" })
    );
    writeFileSync(join(repo, "pnpm-workspace.yaml"), "allowBuilds:\n  esbuild: true\n");
    writeFileSync(join(repo, ".nvmrc"), "26\n");
    const result = run(appDir, ["--no-register", "--force"]);
    expect(result.status, result.stderr).toBe(0);
    expect(pnpmStep(build())).toBeDefined();
    expect(pnpmStep(build())?.with).toBeUndefined();
    expect(nodeStep(build())?.with).toEqual({ "node-version-file": ".nvmrc" });
    expect(result.stderr).not.toContain("pins no pnpm");
    expect(result.stderr).not.toContain("onlyBuiltDependencies");
  });

  it("names the service account from --service-account", () => {
    const result = run(repo, [
      "--app",
      "acme/sales",
      "--service-account",
      "acme/release-bot",
      "--no-register"
    ]);
    expect(result.status, result.stderr).toBe(0);
    const written = readFileSync(join(repo, WORKFLOW_PATH), "utf8");
    expect(written).not.toContain("setup-oxyc");
    expect(written).toContain(
      "OXY_SERVICE_ACCOUNT: <service-account-id>   # REPLACE with the ID of acme/release-bot"
    );
    expect(written).not.toContain("OXY_SERVICE_ACCOUNT: acme/");
    expect(result.stderr).toContain("--no-register was passed");
    expect(result.stdout).toContain("create the service account `release-bot`");
  });

  it("uses the setup-oxyc action only with --setup-action, and says it is not published", () => {
    const result = run(repo, [
      "--app",
      "acme/sales",
      "--env",
      "dev",
      "--setup-action",
      "--no-register"
    ]);
    expect(result.status, result.stderr).toBe(0);
    const written = readFileSync(join(repo, WORKFLOW_PATH), "utf8");
    expect(written).toContain(`uses: ${SETUP_ACTION}`);
    expect(written).toContain(
      "service-account: <service-account-id>   # REPLACE with the ID of acme/deployer"
    );
    expect(written).not.toContain("service-account: acme/");
    // `--env dev` resolved to the deployment the action exchanges against.
    expect(written).toContain("host: https://aip.dev.oxy.tech");
    expect(written).not.toContain("OXY_SERVICE_ACCOUNT");
    expect(result.stderr).toContain("not published yet");
    expect(result.stderr).toContain("without --setup-action");
  });

  it("no longer knows --no-setup-action: the default it named is the default now", () => {
    const result = run(repo, ["--app", "acme/sales", "--no-setup-action", "--no-register"]);
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("--no-setup-action");
  });

  it("with --target, exchanges and publishes at that one deployment", () => {
    const result = run(repo, [
      "--app",
      "acme/sales",
      "--target",
      "https://oxy.acme.test",
      "--no-register"
    ]);
    expect(result.status, result.stderr).toBe(0);
    const written = readFileSync(join(repo, WORKFLOW_PATH), "utf8");
    // Not `--env production`: the token would be minted for one host and sent to another.
    expect(written).toContain("--env https://oxy.acme.test --org acme --app sales");
    expect(written).not.toContain("--env production");

    // With the action, the same deployment is also where it exchanges.
    const withAction = run(repo, [
      "--app",
      "acme/sales",
      "--target",
      "https://oxy.acme.test",
      "--setup-action",
      "--no-register",
      "--force"
    ]);
    expect(withAction.status, withAction.stderr).toBe(0);
    const rewritten = readFileSync(join(repo, WORKFLOW_PATH), "utf8");
    expect(rewritten).toContain("host: https://oxy.acme.test");
    expect(rewritten).toContain("--env https://oxy.acme.test --org acme --app sales");
  });

  it("refuses a service account name the server would, before writing anything", () => {
    for (const name of ["acme/Release_Bot", "acme/1bot", "x"]) {
      const result = run(repo, ["--app", "acme/sales", "--service-account", name, "--no-register"]);
      expect(result.status, name).toBe(ExitCode.USAGE);
      expect(result.stderr, name).toContain("not a valid service account name");
    }
    expect(existsSync(join(repo, WORKFLOW_PATH))).toBe(false);
  });

  it("refuses a service account of another org: its grants could never reach this app", () => {
    const result = run(repo, [
      "--app",
      "acme/sales",
      "--service-account",
      "globex/deployer",
      "--no-register"
    ]);
    expect(result.status).toBe(ExitCode.USAGE);
    expect(result.stderr).toContain("only its own organization");
    expect(result.stderr).toContain("--service-account acme/deployer");
    expect(existsSync(join(repo, WORKFLOW_PATH))).toBe(false);
  });

  it("refuses to overwrite without --force", () => {
    mkdirSync(dirname(join(repo, WORKFLOW_PATH)), { recursive: true });
    writeFileSync(join(repo, WORKFLOW_PATH), "mine");
    const refused = run(repo, ["--app", "acme/sales"]);
    expect(refused.status).toBe(ExitCode.REFUSED);
    expect(readFileSync(join(repo, WORKFLOW_PATH), "utf8")).toBe("mine");

    expect(run(repo, ["--app", "acme/sales", "--force"]).status).toBe(0);
    expect(readFileSync(join(repo, WORKFLOW_PATH), "utf8")).toContain("publish --prebuilt");
  });

  it("asks which app when neither --app nor a manifest says", () => {
    const result = run(repo, []);
    expect(result.status).toBe(ExitCode.USAGE);
    expect(existsSync(join(repo, WORKFLOW_PATH))).toBe(false);
  });
});
