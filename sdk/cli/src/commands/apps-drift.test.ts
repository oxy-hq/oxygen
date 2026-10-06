/**
 * `oxyc apps drift`: every outcome, against real git repositories in a temp
 * directory.
 *
 * REAL GIT, not a stub: what is under test is whether the questions asked of a
 * checkout ("do you have this commit", "is it on this branch", "what touched
 * this directory since") are the right questions, and a stubbed `git` would
 * only confirm that the code calls what the test says it calls.
 *
 * The property every case below pins: a comparison that could not be made is
 * `unknown` with a reason — never `in_sync`.
 */

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import type { BuildHistory } from "./apps-client.js";
import { type DriftReport, evaluateDrift, runAppsDrift } from "./apps-drift.js";

const SCRATCH: string[] = [];
afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

/** Run git in `dir`, ignoring the developer's own config (signing, hooks, fsmonitor). */
function git(dir: string, ...args: string[]): string {
  const result = spawnSync("git", args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" }
  });
  if (result.status !== 0) throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout.trim();
}

/** A fresh repository on `main`, inside its own scratch parent directory. */
function makeRepo(name = "checkout"): string {
  const parent = mkdtempSync(join(tmpdir(), "oxyc-drift-"));
  SCRATCH.push(parent);
  const dir = join(parent, name);
  mkdirSync(dir);
  git(dir, "init", "--quiet", "--initial-branch=main");
  git(dir, "config", "user.name", "Test");
  git(dir, "config", "user.email", "test@oxy.test");
  git(dir, "config", "commit.gpgsign", "false");
  return dir;
}

/** Write `files`, commit everything, and return the commit id. */
function commit(dir: string, message: string, files: Record<string, string>): string {
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  git(dir, "add", "--all");
  git(dir, "commit", "--quiet", "--message", message);
  return git(dir, "rev-parse", "HEAD");
}

const APP = { id: "aaaaaaaa-1111-4111-8111-111111111111", org_slug: "acme", slug: "store" };
const REPO = "oxy-hq/customer-apps";

/**
 * The app lives in a folder named after NEITHER slug — the real layout this
 * command has to cope with (`apps/pokehouse/…` holds the org `poke-house`).
 */
const APP_DIR = "apps/acme-corp/storefront";
const MANIFEST = JSON.stringify({ slug: "store", orgSlug: "acme" });

/** A repository whose first commit is the one that was published. */
function publishedRepo(manifestPath = `${APP_DIR}/oxy-app.json`): { dir: string; sha: string } {
  const dir = makeRepo();
  const sha = commit(dir, "publish the store app", {
    [manifestPath]: MANIFEST,
    [`${APP_DIR}/src/main.ts`]: "export const version = 1;\n",
    "README.md": "# apps\n"
  });
  return { dir, sha };
}

function history(live: Partial<BuildHistory["builds"][number]> | null): BuildHistory {
  const base = {
    id: "build-row",
    build_id: "build-1",
    created_at: "2026-09-10T08:00:00+00:00",
    is_draft: false,
    is_published: true,
    published_by_email: "ada@oxy.test",
    published_via: null,
    source_repo: `git@github.com:${REPO}.git`,
    commit_sha: null,
    source_branch: "main"
  };
  return {
    builds:
      live === null ? [{ ...base, is_published: false, is_draft: true }] : [{ ...base, ...live }],
    promoted_at: null,
    promoted_by_email: null
  };
}

/** Evaluate against one checkout, answering only for the repository it is a clone of. */
function drift(dir: string | undefined, live: Parameters<typeof history>[0]): DriftReport {
  return evaluateDrift(APP, history(live), (repo) => (repo === REPO ? dir : undefined));
}

beforeEach(() => {
  // The code under test runs git with the process environment; keep the
  // developer's global config (fsmonitor, signing) out of the scratch repos.
  vi.stubEnv("GIT_CONFIG_GLOBAL", "/dev/null");
  vi.stubEnv("GIT_CONFIG_NOSYSTEM", "1");
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("evaluateDrift — a comparison that was made", () => {
  it("is in sync when nothing touched the app directory after the published commit", () => {
    const { dir, sha } = publishedRepo();
    // A later commit elsewhere in the repository is not drift of THIS app.
    commit(dir, "docs: unrelated", { "README.md": "# apps, edited\n" });

    const report = drift(dir, { commit_sha: sha });

    expect(report).toMatchObject({
      status: "in_sync",
      repo: REPO,
      app_dir: APP_DIR,
      commits: [],
      checkout: { path: dir, branch: "main" }
    });
    expect(report.reason).toBeUndefined();
  });

  it("counts and lists the commits that touch the app directory, newest first", () => {
    const { dir, sha } = publishedRepo();
    commit(dir, "feat: second version", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });
    commit(dir, "docs: unrelated", { "README.md": "# apps, edited\n" });
    const newest = commit(dir, "fix: third version", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 3;\n"
    });

    const report = drift(dir, { commit_sha: sha });

    expect(report.status).toBe("ahead");
    expect(report.commits?.map((c) => c.subject)).toEqual([
      "fix: third version",
      "feat: second version"
    ]);
    expect(report.commits?.[0]).toMatchObject({ sha: newest, author: "Test" });
  });

  it("accepts the abbreviated commit id some builds record", () => {
    const { dir, sha } = publishedRepo();
    expect(drift(dir, { commit_sha: sha.slice(0, 7) }).status).toBe("in_sync");
  });

  it("finds the app when its manifest is at <app>/public/oxy-app.json", () => {
    const { dir, sha } = publishedRepo(`${APP_DIR}/public/oxy-app.json`);
    // Outside `public/`, inside the app: this must count.
    commit(dir, "feat: change the source", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });

    const report = drift(dir, { commit_sha: sha });

    expect(report.app_dir).toBe(APP_DIR);
    expect(report.status).toBe("ahead");
    expect(report.commits).toHaveLength(1);
  });

  /** "Read-only" is a promise about the user's checkout, so it is checked on one. */
  it("leaves the checkout exactly as it found it", () => {
    const { dir, sha } = publishedRepo();
    commit(dir, "feat: second version", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });
    const index = join(dir, ".git", "index");
    const before = {
      head: git(dir, "rev-parse", "HEAD"),
      index: readFileSync(index),
      mtime: statSync(index).mtimeMs
    };

    drift(dir, { commit_sha: sha });

    expect(git(dir, "rev-parse", "HEAD")).toBe(before.head);
    expect(readFileSync(index).equals(before.index)).toBe(true);
    expect(statSync(index).mtimeMs).toBe(before.mtime);
    expect(git(dir, "status", "--porcelain")).toBe("");
  });
});

describe("evaluateDrift — a comparison that could not be made is unknown, never in sync", () => {
  it("nothing is live", () => {
    const { dir } = publishedRepo();
    expect(drift(dir, null)).toMatchObject({ status: "unknown", reason: "not_published" });
  });

  it("the live build records no commit", () => {
    const { dir } = publishedRepo();
    const report = drift(dir, { commit_sha: null });
    expect(report).toMatchObject({ status: "unknown", reason: "source_unrecorded" });
    expect(report.detail).toContain("records no commit");
    // It stopped before looking at any checkout.
    expect(report.checkout).toBeUndefined();
  });

  it("the live build records no repository", () => {
    const { dir, sha } = publishedRepo();
    const report = drift(dir, { commit_sha: sha, source_repo: null });
    expect(report).toMatchObject({ status: "unknown", reason: "source_unrecorded" });
    expect(report.detail).toContain("records no repository");
  });

  it("the repository is not checked out on this machine", () => {
    const report = drift(undefined, { commit_sha: "0123456789abcdef0123456789abcdef01234567" });
    expect(report).toMatchObject({ status: "unknown", reason: "repo_not_checked_out", repo: REPO });
    expect(report.detail).toContain(`gh repo clone ${REPO}`);
  });

  it("the checkout does not have the published commit", () => {
    const { dir } = publishedRepo();
    const report = drift(dir, { commit_sha: "0123456789abcdef0123456789abcdef01234567" });
    expect(report).toMatchObject({ status: "unknown", reason: "commit_not_in_checkout" });
    expect(report.detail).toContain("git fetch");
  });

  it("no oxy-app.json declares this app", () => {
    const dir = makeRepo();
    // Same slug, a different organization: the staging copy of the app must
    // not be compared against the production app's directory by slug alone.
    const sha = commit(dir, "publish", {
      [`${APP_DIR}/oxy-app.json`]: JSON.stringify({ slug: "store", orgSlug: "acme-staging" })
    });

    const report = drift(dir, { commit_sha: sha });

    expect(report).toMatchObject({ status: "unknown", reason: "app_dir_not_found" });
    expect(report.detail).toContain(`${APP_DIR}/oxy-app.json declares orgSlug "acme-staging"`);
    expect(report.app_dir).toBeUndefined();
  });

  it("two directories declare this app", () => {
    const dir = makeRepo();
    const sha = commit(dir, "publish", {
      "apps/one/oxy-app.json": MANIFEST,
      "apps/two/oxy-app.json": MANIFEST
    });

    expect(drift(dir, { commit_sha: sha })).toMatchObject({
      status: "unknown",
      reason: "app_dir_ambiguous"
    });
  });

  it("the published commit is not on the checked-out branch", () => {
    const { dir } = publishedRepo();
    git(dir, "checkout", "--quiet", "-b", "feature");
    const sha = commit(dir, "feat: only on the feature branch", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });
    git(dir, "checkout", "--quiet", "main");

    const report = drift(dir, { commit_sha: sha, source_branch: "feature" });

    // `sha..HEAD` is empty here, so counting alone would have said "in sync".
    expect(report).toMatchObject({ status: "unknown", reason: "commit_not_on_branch" });
    expect(report.detail).toContain("published from branch feature");
  });

  it("a tracked file in the app directory is modified", () => {
    const { dir, sha } = publishedRepo();
    writeFileSync(join(dir, APP_DIR, "src/main.ts"), "export const version = 99;\n");

    const report = drift(dir, { commit_sha: sha });

    // No commit after the published one — and still not "in sync".
    expect(report).toMatchObject({ status: "unknown", reason: "working_tree_dirty", commits: [] });
  });

  it("the app directory holds an untracked file", () => {
    const { dir, sha } = publishedRepo();
    writeFileSync(join(dir, APP_DIR, "src/new.ts"), "export {};\n");
    expect(drift(dir, { commit_sha: sha })).toMatchObject({
      status: "unknown",
      reason: "working_tree_dirty"
    });
  });

  it("`git status` fails in the checkout, so whether the tree is dirty cannot be read", () => {
    const { dir, sha } = publishedRepo();
    // A setting only `git status` reads. `git log`, `ls-files` and `merge-base`
    // still succeed, so every earlier step passes and no commit follows the
    // published one — the exact case that used to come back as `in_sync`.
    git(dir, "config", "status.showUntrackedFiles", "bogus");

    const report = drift(dir, { commit_sha: sha });

    expect(report).toMatchObject({ status: "unknown", reason: "git_failed" });
    expect(report.detail).toContain("git status failed");
    expect(report.detail).toContain("bogus");
    expect(report.commits).toBeUndefined();
  });

  it("uncommitted changes outside the app directory do not count", () => {
    const { dir, sha } = publishedRepo();
    writeFileSync(join(dir, "README.md"), "# edited, not committed\n");
    expect(drift(dir, { commit_sha: sha }).status).toBe("in_sync");
  });

  it("reports the commits it found alongside the uncommitted changes", () => {
    const { dir, sha } = publishedRepo();
    commit(dir, "feat: second version", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });
    writeFileSync(join(dir, APP_DIR, "src/main.ts"), "export const version = 3;\n");

    const report = drift(dir, { commit_sha: sha });

    expect(report).toMatchObject({ status: "unknown", reason: "working_tree_dirty" });
    expect(report.commits).toHaveLength(1);
    expect(report.detail).toContain("on top of 1 commit(s)");
  });
});

// ── the command: requests, checkout discovery, exit codes ──────────────────

const TARGET = "https://oxy.test";
const LIST = "/api/customer-apps";

function fakeContext(cwd: string): Context {
  return {
    cwd,
    flags: { env: "production", tokenEnv: "OXY_TOKEN", apiKeyEnv: "OXY_API_KEY" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => "tok",
    maybeBearer: async () => "tok",
    storedBearer: () => "tok",
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext(cwd)
  };
}

function stubFetch(routes: Record<string, { status: number; body: unknown }>): string[] {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL, init?: RequestInit) => {
      const path = String(input).replace(TARGET, "");
      calls.push(`${(init?.method ?? "GET").toUpperCase()} ${path}`);
      const reply = routes[path] ?? { status: 404, body: { error: "no stub" } };
      return new Response(JSON.stringify(reply.body), { status: reply.status });
    })
  );
  return calls;
}

function capture(): { stdout: () => string; stderr: () => string } {
  const chunks = { out: [] as string[], err: [] as string[] };
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    chunks.out.push(String(chunk));
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    chunks.err.push(String(chunk));
    return true;
  });
  return { stdout: () => chunks.out.join(""), stderr: () => chunks.err.join("") };
}

const row = (id: string, org: string, slug: string, publishedAt: string | null) => ({
  id,
  slug,
  org_slug: org,
  name: slug,
  published_at: publishedAt
});
const ID_OTHER = "bbbbbbbb-2222-4222-8222-222222222222";
const ID_DRAFT = "cccccccc-3333-4333-8333-333333333333";

function listing() {
  return {
    status: 200,
    body: {
      items: [
        row(APP.id, "acme", "store", "2026-09-10T08:00:00+00:00"),
        row(ID_OTHER, "globex", "portal", "2026-09-11T08:00:00+00:00"),
        row(ID_DRAFT, "acme", "unpublished", null)
      ],
      next_offset: null
    }
  };
}

describe("apps drift — the command", () => {
  /** Point checkout discovery at scratch directories instead of the developer's home. */
  function isolateDiscovery(root: string): void {
    const state = mkdtempSync(join(tmpdir(), "oxyc-drift-state-"));
    SCRATCH.push(state);
    vi.stubEnv("OXYC_REPO_ROOTS", root);
    vi.stubEnv("OXYC_CACHE_DIR", join(state, "cache"));
    vi.stubEnv("OXYC_DOSSIER_ROOT", join(state, "dossiers"));
  }

  it("finds the checkout by its origin remote and compares one app", async () => {
    const { dir, sha } = publishedRepo();
    git(dir, "remote", "add", "origin", `https://github.com/${REPO}.git`);
    isolateDiscovery(dirname(dir));
    const calls = stubFetch({
      [`${LIST}?limit=100&offset=0`]: listing(),
      [`${LIST}/${APP.id}/builds`]: { status: 200, body: history({ commit_sha: sha }) }
    });
    const io = capture();

    await runAppsDrift(fakeContext(dir), "acme/store", { json: true });

    expect(JSON.parse(io.stdout())).toMatchObject({
      app: "acme/store",
      status: "in_sync",
      checkout: { path: dir }
    });
    expect(calls.every((call) => call.startsWith("GET "))).toBe(true);
  });

  it("--dir names the checkout for one app whatever its remote is called", async () => {
    // No `origin` at all: a fork or a fresh clone of a mirror.
    const { dir, sha } = publishedRepo();
    commit(dir, "feat: second version", {
      [`${APP_DIR}/src/main.ts`]: "export const version = 2;\n"
    });
    isolateDiscovery(join(dirname(dir), "nothing-here"));
    stubFetch({
      [`${LIST}?limit=100&offset=0`]: listing(),
      [`${LIST}/${APP.id}/builds`]: { status: 200, body: history({ commit_sha: sha }) }
    });
    const io = capture();

    // A path INSIDE the checkout resolves to its root.
    await runAppsDrift(fakeContext("/"), "acme/store", { dir: join(dir, APP_DIR) });

    expect(io.stdout()).toMatch(/drift\s+1 commit ahead/);
    expect(io.stdout()).toContain("feat: second version");
  });

  it("refuses a --dir that is not a git working tree, before any request", async () => {
    const calls = stubFetch({});
    const outside = mkdtempSync(join(tmpdir(), "oxyc-drift-plain-"));
    SCRATCH.push(outside);

    for (const dir of [outside, join(outside, "missing")]) {
      const error = await runAppsDrift(fakeContext("/"), "acme/store", { dir }).then(
        () => undefined,
        (e: unknown) => e
      );
      expect(error).toBeInstanceOf(CliError);
      expect((error as CliError).code).toBe(ExitCode.USAGE);
    }
    expect(calls).toEqual([]);
  });

  /**
   * With no <app>: published apps only, one builds request each. An app whose
   * builds request FAILED is listed as unknown with that reason, and the
   * command exits with the failure's code — it is not "in sync" and it is not
   * silently missing.
   */
  it("compares every published app, and exits non-zero when a builds request failed", async () => {
    const { dir, sha } = publishedRepo();
    git(dir, "remote", "add", "origin", `git@github.com:${REPO}.git`);
    isolateDiscovery(dirname(dir));
    const calls = stubFetch({
      [`${LIST}?limit=100&offset=0`]: listing(),
      [`${LIST}/${APP.id}/builds`]: { status: 200, body: history({ commit_sha: sha }) },
      [`${LIST}/${ID_OTHER}/builds`]: { status: 502, body: { error: "upstream" } }
    });
    const io = capture();

    const error = await runAppsDrift(fakeContext("/"), undefined, { json: true }).then(
      () => undefined,
      (e: unknown) => e
    );

    const reports = JSON.parse(io.stdout()) as DriftReport[];
    expect(reports.map((r) => [r.app, r.status, r.reason])).toEqual([
      ["acme/store", "in_sync", undefined],
      ["globex/portal", "unknown", "request_failed"]
    ]);
    expect(error).toBeInstanceOf(CliError);
    expect((error as CliError).code).toBe(ExitCode.UNAVAILABLE);
    // The draft app has nothing live, so it cost no builds request.
    expect(calls).not.toContain(`GET ${LIST}/${ID_DRAFT}/builds`);
  });

  it("renders unknown with its reason in the table, and counts it apart from in sync", async () => {
    const { dir, sha } = publishedRepo();
    git(dir, "remote", "add", "origin", `https://github.com/${REPO}.git`);
    isolateDiscovery(dirname(dir));
    stubFetch({
      [`${LIST}?limit=100&offset=0`]: listing(),
      [`${LIST}/${APP.id}/builds`]: { status: 200, body: history({ commit_sha: sha }) },
      [`${LIST}/${ID_OTHER}/builds`]: {
        status: 200,
        body: history({ commit_sha: sha, source_repo: "https://github.com/oxy-hq/elsewhere.git" })
      }
    });
    const io = capture();

    await runAppsDrift(fakeContext("/"), undefined, {});

    expect(io.stdout()).toMatch(/\| acme\/store \| in sync \|/);
    expect(io.stdout()).toMatch(
      /\| globex\/portal \| unknown — repository not checked out here \|/
    );
    expect(io.stderr()).toContain("2 published app(s): 1 in sync, 0 ahead, 1 unknown");
    expect(io.stderr()).toContain("1 draft app(s) not compared");
  });

  it("a git failure in one checkout is that app's unknown, and the other apps still get a row", async () => {
    const { dir, sha } = publishedRepo();
    git(dir, "remote", "add", "origin", `https://github.com/${REPO}.git`);
    // Only `git status` reads this, so the failure comes at the last step.
    git(dir, "config", "status.showUntrackedFiles", "bogus");
    isolateDiscovery(dirname(dir));
    stubFetch({
      [`${LIST}?limit=100&offset=0`]: listing(),
      [`${LIST}/${APP.id}/builds`]: { status: 200, body: history({ commit_sha: sha }) },
      [`${LIST}/${ID_OTHER}/builds`]: {
        status: 200,
        body: history({ commit_sha: sha, source_repo: "https://github.com/oxy-hq/elsewhere.git" })
      }
    });
    const io = capture();

    // It must resolve: a broken checkout is a finding about one app, not a
    // reason to print no report at all.
    await runAppsDrift(fakeContext("/"), undefined, { json: true });

    const reports = JSON.parse(io.stdout()) as DriftReport[];
    expect(reports.map((r) => [r.app, r.status, r.reason])).toEqual([
      ["acme/store", "unknown", "git_failed"],
      ["globex/portal", "unknown", "repo_not_checked_out"]
    ]);
  });
});
