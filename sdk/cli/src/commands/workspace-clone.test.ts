/**
 * `oxyc workspace clone`: the argument rules, the no-repository path, where the
 * workspace sits after a clone, and the rule that an editor is never launched
 * unattended. The API is a stubbed `fetch`; git and the editor are injected
 * `CloneDeps`, so nothing is spawned.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createContext } from "../context/resolve.js";
import { type Routes, stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  type CloneDeps,
  cloneArgs,
  closingLines,
  parseOpen,
  planOpen,
  redactUrl,
  repoNameFromUrl,
  runWorkspaceClone,
  workspacePath
} from "./workspace-clone.js";

const TARGET = "https://oxy.test";
const WS = "3f2c9a1e-5d2b-4c1a-9e0f-7a6b5c4d3e2f";
const ORG = "7d1e2f30-aaaa-4bbb-8ccc-0123456789ab";
const URL_ = "https://github.com/acme/analytics.git";

let scratch: string;
let stdout: string;
let stderr: string;

const context = (flags: { org?: string } = {}) =>
  createContext({ env: "production", target: TARGET, ...flags }, scratch);

const meta = (extra: Record<string, unknown> = {}): Routes => ({
  [`GET /api/${WS}/meta`]: () => ({
    status: 200,
    body: {
      id: WS,
      name: "Analytics",
      git_remote_url: URL_,
      default_branch: "main",
      repo_subdir: null,
      ...extra
    }
  })
});

interface Fake extends CloneDeps {
  gitCalls: string[][];
  launched: [string, string[]][];
}

function fake(over: Partial<CloneDeps> & { gitStatus?: number } = {}): Fake {
  const gitCalls: string[][] = [];
  const launched: [string, string[]][] = [];
  const { gitStatus = 0, ...rest } = over;
  return {
    gitCalls,
    launched,
    git: (args) => {
      gitCalls.push(args);
      return gitStatus;
    },
    launch: (command, args) => {
      launched.push([command, args]);
    },
    onPath: () => true,
    interactive: () => true,
    listDir: () => undefined,
    isDir: () => true,
    ...rest
  };
}

async function failure(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-ws-clone-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "oxy_pat_secret");
  vi.stubEnv("OXYC_QUIET", "");
  stdout = "";
  stderr = "";
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    stdout += String(chunk);
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    stderr += String(chunk);
    return true;
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("argument parsing", () => {
  it("accepts code, cursor, none and nothing; refuses the rest as a usage error", () => {
    expect(parseOpen(undefined)).toBeUndefined();
    for (const v of ["code", "cursor", "none"] as const) expect(parseOpen(v)).toBe(v);
    let cause: unknown;
    try {
      parseOpen("vim");
    } catch (thrown) {
      cause = thrown;
    }
    expect((cause as CliError).code).toBe(ExitCode.USAGE);
  });

  it("names the default directory after the repository", () => {
    expect(repoNameFromUrl(URL_)).toBe("analytics");
    expect(repoNameFromUrl("git@github.com:acme/analytics.git")).toBe("analytics");
    expect(repoNameFromUrl("https://github.com/acme/analytics/")).toBe("analytics");
  });

  it("builds git clone with --branch only when a branch is known, and never rewrites the URL", () => {
    expect(cloneArgs(URL_, "/w/analytics", "main")).toEqual([
      "clone",
      "--branch",
      "main",
      URL_,
      "/w/analytics"
    ]);
    expect(cloneArgs(URL_, "/w/analytics", undefined)).toEqual(["clone", URL_, "/w/analytics"]);
  });

  it("redacts credentials for display only", () => {
    expect(redactUrl("https://u:pw@github.com/a/b.git")).toBe("https://github.com/a/b.git");
    expect(redactUrl("git@github.com:a/b.git")).toBe("git@github.com:a/b.git");
  });

  it("uses --dir, --branch and an id; a name needs --org", async () => {
    stubFetch(TARGET, meta());
    const deps = fake();
    await runWorkspaceClone(
      context(),
      WS,
      { dir: join(scratch, "x"), branch: "dev", open: "none" },
      deps
    );
    expect(deps.gitCalls[0]).toEqual(["clone", "--branch", "dev", URL_, join(scratch, "x")]);

    const bad = await failure(runWorkspaceClone(context(), "Analytics", {}, fake()));
    expect(bad.code).toBe(ExitCode.USAGE);
  });

  it("resolves a name among the org's workspaces", async () => {
    const calls = stubFetch(TARGET, {
      ...meta(),
      "GET /api/orgs": () => ({ status: 200, body: [{ id: ORG, slug: "acme" }] }),
      [`GET /api/orgs/${ORG}/workspaces`]: () => ({
        status: 200,
        body: [
          { id: "11111111-1111-4111-8111-111111111111", name: "Other" },
          { id: WS, name: "Analytics" }
        ]
      })
    });
    const deps = fake();
    await runWorkspaceClone(context({ org: "acme" }), "analytics", { open: "none" }, deps);
    expect(calls.map((c) => c.path)).toEqual([
      "/api/orgs",
      `/api/orgs/${ORG}/workspaces`,
      `/api/${WS}/meta`
    ]);
    expect(deps.gitCalls).toHaveLength(1);
  });
});

describe("a workspace with no repository", () => {
  it("exits not-found with one sentence and runs nothing", async () => {
    stubFetch(TARGET, meta({ git_remote_url: null, default_branch: null }));
    const deps = fake();
    const cause = await failure(runWorkspaceClone(context(), WS, {}, deps));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.message).toBe(
      "workspace Analytics has no repository; connect one in the workspace's Settings"
    );
    expect(deps.gitCalls).toEqual([]);
    expect(deps.launched).toEqual([]);
    expect(stdout).toBe("");
  });
});

describe("the target directory and a failed clone", () => {
  it("refuses a non-empty directory with the refusal exit code", async () => {
    stubFetch(TARGET, meta());
    const deps = fake({ listDir: () => ["README.md"] });
    const cause = await failure(runWorkspaceClone(context(), WS, { dir: "taken" }, deps));
    expect(cause.code).toBe(ExitCode.REFUSED);
    expect(deps.gitCalls).toEqual([]);
  });

  it("accepts an empty existing directory", async () => {
    stubFetch(TARGET, meta());
    const deps = fake({ listDir: () => [] });
    await runWorkspaceClone(context(), WS, { dir: join(scratch, "empty"), open: "none" }, deps);
    expect(deps.gitCalls).toHaveLength(1);
  });

  it("exits with the request code when git fails, and opens nothing", async () => {
    stubFetch(TARGET, meta());
    const deps = fake({ gitStatus: 128 });
    const cause = await failure(runWorkspaceClone(context(), WS, {}, deps));
    expect(cause.code).toBe(ExitCode.REQUEST);
    expect(deps.launched).toEqual([]);
    expect(stdout).toBe("");
  });
});

describe("the workspace's subdirectory", () => {
  it("is the repository root with no subdir", () => {
    expect(workspacePath("/w/r", undefined)).toBe("/w/r");
    expect(workspacePath("/w/r", "")).toBe("/w/r");
    expect(workspacePath("/w/r", "/")).toBe("/w/r");
  });

  it("joins a nested subdir and refuses one that climbs out", () => {
    expect(workspacePath("/w/r", "oxy/analytics")).toBe(join("/w/r", "oxy", "analytics"));
    expect(() => workspacePath("/w/r", "../etc")).toThrow(CliError);
    expect(() => workspacePath("/w/r", "a/../../b")).toThrow(CliError);
  });

  it("prints and opens the subdirectory, not the repository root", async () => {
    stubFetch(TARGET, meta({ repo_subdir: "oxy" }));
    const dir = join(scratch, "repo");
    const deps = fake();
    await runWorkspaceClone(context(), WS, { dir, open: "code" }, deps);
    expect(deps.launched).toEqual([["code", [join(dir, "oxy")]]]);
    expect(stdout.split("\n")[0]).toBe(join(dir, "oxy"));
  });

  it("warns and falls back to the root when the subdirectory is not in the clone", async () => {
    stubFetch(TARGET, meta({ repo_subdir: "oxy" }));
    const dir = join(scratch, "repo");
    const deps = fake({ isDir: () => false });
    await runWorkspaceClone(context(), WS, { dir, open: "none" }, deps);
    expect(stderr).toContain("oxy/ is not in the clone");
    expect(stdout.split("\n")[0]).toBe(dir);
  });
});

describe("opening an editor", () => {
  it("launches nothing when not interactive, even for an explicit --open", () => {
    const deps = fake({ interactive: () => false });
    expect(planOpen("code", deps)).toEqual({ command: "code", launch: false });
    expect(planOpen("cursor", deps)).toEqual({ command: "cursor", launch: false });
    expect(planOpen(undefined, deps)).toEqual({ command: "code", launch: false });
  });

  it("defaults to code only when it is on PATH and a person is at the terminal", () => {
    expect(planOpen(undefined, fake())).toEqual({ command: "code", launch: true });
    expect(planOpen(undefined, fake({ onPath: () => false }))).toEqual({
      command: "code",
      launch: false
    });
  });

  it("never launches for --open none", () => {
    expect(planOpen("none", fake()).launch).toBe(false);
  });

  it("prints the command instead of running it in a non-interactive run", async () => {
    stubFetch(TARGET, meta());
    const dir = join(scratch, "repo");
    const deps = fake({ interactive: () => false });
    await runWorkspaceClone(context(), WS, { dir, open: "code" }, deps);
    expect(deps.launched).toEqual([]);
    expect(stderr).toContain(`code ${dir}`);
  });

  it("launches cursor when asked, on a terminal", async () => {
    stubFetch(TARGET, meta());
    const dir = join(scratch, "repo");
    const deps = fake();
    await runWorkspaceClone(context(), WS, { dir, open: "cursor" }, deps);
    expect(deps.launched).toEqual([["cursor", [dir]]]);
  });
});

describe("the closing lines", () => {
  it("are the path, how it ships, and how to preview", async () => {
    stubFetch(TARGET, meta());
    const dir = join(scratch, "repo");
    await runWorkspaceClone(context(), WS, { dir, open: "none" }, fake());
    expect(stdout.trimEnd().split("\n")).toEqual(closingLines(dir, "main", WS));
    expect(stdout.split("\n")[1]).toBe("push to main to compile and deploy");
    expect(stdout).toContain("oxyc preview create <branch>");
  });
});

describe("an org that is not yours", () => {
  it("is not found, not a request error", async () => {
    stubFetch(TARGET, {
      "GET /api/orgs": () => ({ status: 200, body: [{ id: ORG, slug: "acme" }] })
    });
    const cause = await failure(
      runWorkspaceClone(context({ org: "other" }), "Analytics", {}, fake())
    );
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
  });
});
