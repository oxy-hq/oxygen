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
  SERVICE_ACCOUNT_ID_PLACEHOLDER,
  SETUP_ACTION,
  WORKFLOW_PATH,
  type WorkflowOptions,
  workflowYaml
} from "./init-ci.js";

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
  it("adds a checks step only when the workflow promotes AND a check is declared", () => {
    const names = (o: WorkflowOptions) => jobs(o).publish.steps.map((s) => s.name ?? s.uses);

    for (const base of [OPTIONS, INLINE]) {
      // `oxyc checks run` errors on an app with no checks, so a step that was
      // always written would fail the first run of every generated workflow.
      expect(names(base)).not.toContain("Run the app's checks");

      // And a check runs against the app's LIVE build, so after a draft publish
      // it would verify the previously promoted code — passing while the build
      // this job uploaded is broken, and failing outright the first time a check
      // is added, because the live build predates the flag.
      expect(names({ ...base, hasChecks: true })).not.toContain("Run the app's checks");
      expect(names({ ...base, promote: true })).not.toContain("Run the app's checks");

      const steps = jobs({ ...base, hasChecks: true, promote: true }).publish.steps;
      expect(steps.at(-2)?.run).toContain("publish --prebuilt --promote");
      const step = steps.at(-1);
      expect(step?.name).toBe("Run the app's checks");
      expect(step?.run).toContain("checks run acme/sales --env production");
      // Nothing is stored: no step ever names a secret.
      expect(JSON.stringify(steps)).not.toContain("secrets.");
    }
  });

  it("runs the checks by app id when it is known", () => {
    // A service account's token may not list apps, so the id spares the
    // command resolving a slug from the token's grants.
    const steps = jobs({ ...OPTIONS, hasChecks: true, promote: true, appId: APP_ID }).publish.steps;
    expect(steps.at(-1)?.run).toBe(`oxyc checks run ${APP_ID} --env production`);
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
      { host: "javascript:alert(1)" },
      { appId: "not-a-uuid; id" }
    ]) {
      expect(() => workflowYaml({ ...OPTIONS, ...bad }), JSON.stringify(bad)).toThrow(CliError);
    }
  });

  it("still accepts a deployment URL as the env and a nested out dir", () => {
    const { publish } = jobs({ ...OPTIONS, env: "https://acme.oxygen-hq.com", outDir: "dist/spa" });
    expect(publish.steps.at(-1)?.run).toContain("--env https://acme.oxygen-hq.com");
  });

  /** pnpm/action-setup refuses two pnpm versions; package.json's pin is the one. */
  it("defers to package.json's pnpm and the repo's .nvmrc when present", () => {
    const { build } = jobs({ ...OPTIONS, pnpmFromPackageJson: true, nodeVersionFile: true });
    expect(build.steps.find((s) => s.uses === "pnpm/action-setup@v4")?.with).toBeUndefined();
    expect(build.steps.find((s) => s.uses === "actions/setup-node@v4")?.with).toEqual({
      "node-version-file": ".nvmrc"
    });
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
    const where = "Organization settings → API access → Service accounts";
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
    writeFileSync(join(appDir, "oxy-app.json"), JSON.stringify({ slug: "sales", orgSlug: "acme" }));

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
