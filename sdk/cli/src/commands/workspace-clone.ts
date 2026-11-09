/**
 * `oxyc workspace clone <workspace>` — a workspace's repository on your disk,
 * ready to edit with your own tools.
 *
 * AUTHORING HAPPENS IN THE REPOSITORY, NOT IN THE PRODUCT. The Factory (the
 * in-app IDE) is retiring (`internal-docs/factory-retirement.md`, phase 5): you
 * clone, edit with VS Code, Cursor or an agent, and push; a push to the default
 * branch compiles and deploys. This is that first step, and the in-app "work in
 * your editor" panel prints exactly this command.
 *
 * ONE ROUTE TELLS US WHERE THE REPOSITORY IS: `GET /api/{workspace}/meta`
 * (`git_remote_url`, `default_branch`, `repo_subdir`), a Postgres-only read any
 * replica serves. Everything after that is the user's own `git` and the
 * credentials it already has — no token is injected and the URL is not
 * rewritten, so a repository you cannot clone with `git clone` you cannot clone
 * here, and git's own message says why.
 *
 * AN EDITOR IS NEVER LAUNCHED UNATTENDED. An agent or a CI job running this
 * would otherwise start a GUI on a machine nobody is looking at; without a
 * terminal on both ends the command is printed instead.
 */

import { spawn, spawnSync } from "node:child_process";
import { accessSync, constants, existsSync, readdirSync, statSync } from "node:fs";
import { delimiter, join, resolve } from "node:path";

import { normalizePath } from "../api/paths.js";
import { errorForResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, refusal, usageError } from "../util/errors.js";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export type OpenChoice = "code" | "cursor" | "none";

export interface CloneFlags {
  dir?: string;
  open?: string;
  branch?: string;
}

/** What `/meta` says about the repository; every field may be absent. */
export interface RepoFacts {
  workspaceId: string;
  name: string;
  remoteUrl?: string;
  defaultBranch?: string;
  repoSubdir?: string;
}

/** The side effects, injectable so the tests spawn nothing. */
export interface CloneDeps {
  /** Runs `git clone …` with the terminal inherited; resolves to its exit status. */
  git: (args: string[]) => number;
  /** Starts an editor, detached. */
  launch: (command: string, args: string[]) => void;
  /** Is `command` on PATH? */
  onPath: (command: string) => boolean;
  /** True when a person is at both ends of the terminal. */
  interactive: () => boolean;
  /** `undefined` = does not exist, otherwise the entries inside (a file is `["<file>"]`). */
  listDir: (path: string) => string[] | undefined;
  /** Does the checked-out workspace directory exist? */
  isDir: (path: string) => boolean;
}

export function realDeps(): CloneDeps {
  return {
    git: (args) => spawnSync("git", args, { stdio: "inherit" }).status ?? 1,
    launch: (command, args) => {
      const child = spawn(command, args, { detached: true, stdio: "ignore" });
      child.on("error", () => undefined);
      child.unref();
    },
    onPath: (command) => onPath(command, process.env),
    interactive: () => Boolean(process.stdout.isTTY && process.stdin.isTTY),
    listDir: (path) => {
      if (!existsSync(path)) return undefined;
      return statSync(path).isDirectory() ? readdirSync(path) : ["<file>"];
    },
    isDir: (path) => existsSync(path) && statSync(path).isDirectory()
  };
}

function onPath(command: string, env: NodeJS.ProcessEnv): boolean {
  const exts = process.platform === "win32" ? (env.PATHEXT ?? ".EXE;.CMD").split(";") : [""];
  for (const dir of (env.PATH ?? "").split(delimiter).filter(Boolean)) {
    for (const ext of exts) {
      try {
        accessSync(join(dir, command + ext), constants.X_OK);
        return true;
      } catch {
        // not here; keep walking PATH
      }
    }
  }
  return false;
}

/** `--open`, validated. `undefined` means "not given": the default applies. */
export function parseOpen(value: string | undefined): OpenChoice | undefined {
  if (value === undefined) return undefined;
  if (value === "code" || value === "cursor" || value === "none") return value;
  throw usageError(`--open ${JSON.stringify(value)} is not code, cursor or none`);
}

/** `acme/analytics` out of `https://github.com/acme/analytics.git` or `git@github.com:acme/analytics.git`. */
export function repoNameFromUrl(url: string): string {
  const last = url
    .trim()
    .replace(/[/\\]+$/, "")
    .split(/[/:\\]/)
    .pop();
  const name = (last ?? "").replace(/\.git$/i, "");
  return name && name !== "." && name !== ".." ? name : "workspace";
}

/** `https://user:secret@host/x` -> `https://host/x`, for printing only. */
export function redactUrl(url: string): string {
  return url.replace(/^([a-z][a-z0-9+.-]*:\/\/)[^/@]*@/i, "$1");
}

/**
 * The workspace's own directory inside the checkout.
 *
 * `repo_subdir` is server-supplied and joined onto a path we then open, so it
 * is held to a relative, `/`-separated path with no `..` — anything else is a
 * repository we refuse to point an editor at.
 */
export function workspacePath(repoDir: string, repoSubdir: string | undefined): string {
  const parts = (repoSubdir ?? "").split("/").filter((p) => p !== "" && p !== ".");
  if (parts.some((p) => p === ".." || p.includes("\\") || p.includes(":"))) {
    throw new CliError(
      `the workspace's repo_subdir ${JSON.stringify(repoSubdir)} is not a safe path`,
      {
        code: ExitCode.FAILURE,
        hint: "it must be a relative path inside the repository; clone without it and open the folder yourself"
      }
    );
  }
  return parts.length === 0 ? repoDir : join(repoDir, ...parts);
}

/** `git clone` argv. Branch omitted only when nothing names one. */
export function cloneArgs(url: string, dir: string, branch: string | undefined): string[] {
  return ["clone", ...(branch ? ["--branch", branch] : []), url, dir];
}

/** Which editor to open, or `none`, and whether it will really be launched. */
export interface OpenPlan {
  command: OpenChoice;
  launch: boolean;
}

/**
 * The `--open` decision. Default: `code` when it is on PATH and a person is at
 * the terminal, else nothing is launched. An explicit choice is honoured only
 * when interactive — never launch an editor in a non-interactive run.
 */
export function planOpen(choice: OpenChoice | undefined, deps: CloneDeps): OpenPlan {
  if (choice === "none") return { command: "none", launch: false };
  const interactive = deps.interactive();
  if (choice === undefined) {
    const canLaunch = interactive && deps.onPath("code");
    return { command: "code", launch: canLaunch };
  }
  return { command: choice, launch: interactive && deps.onPath(choice) };
}

function requireTargetDir(deps: CloneDeps, dir: string): void {
  const entries = deps.listDir(dir);
  if (entries === undefined || entries.length === 0) return;
  throw refusal(`${dir} already exists and is not empty`, {
    hint: "pass --dir <path> to clone somewhere else, or move it aside — a clone never writes into a folder with files in it"
  });
}

/** A 2xx JSON body, or the error its status deserves (the exit-code contract's mapping). */
async function getJson(ctx: Context, path: string): Promise<unknown> {
  const response = await request({
    target: ctx.target(),
    path,
    method: "GET",
    bearer: await ctx.bearer()
  });
  if (response.status < 200 || response.status >= 300) throw errorForResponse(response);
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`GET ${path} did not return JSON`, { code: ExitCode.FAILURE });
  }
  return parsed;
}

/**
 * The org's id. `/api/orgs/{org_id}/…` takes a UUID only, while `--org` is a
 * slug, so a slug is matched against `GET /api/orgs` (what `oxyc api {org}/…`
 * does). Unlike `publish`'s best-effort lookup this one lets a 401 or 5xx
 * surface with its own exit code instead of reading as "no such workspace".
 */
async function resolveOrgId(ctx: Context, org: string): Promise<string> {
  if (UUID.test(org)) return org;
  const rows = (await getJson(ctx, "/api/orgs")) as { id?: string; slug?: string }[];
  const match = Array.isArray(rows) ? rows.find((r) => r.slug === org) : undefined;
  if (match?.id) return match.id;
  throw new CliError(`no org with the slug ${JSON.stringify(org)} that you belong to`, {
    code: ExitCode.NOT_FOUND,
    hint: "`oxyc api orgs --jq '.[].slug'` lists yours"
  });
}

/**
 * The workspace's id: a UUID is used as it is; anything else is a NAME, looked
 * up among `--org`'s workspaces (case-insensitively, and it must be unambiguous).
 * A workspace has no slug of its own — `oxyc api {org}/workspaces` is how ids are found.
 */
export async function resolveWorkspaceId(ctx: Context, arg: string): Promise<string> {
  const given = arg.trim();
  if (UUID.test(given)) return given.toLowerCase();
  const org = ctx.placeholders().org;
  if (!org) {
    throw usageError(
      `${JSON.stringify(arg)} is not a workspace id, and there is no org to look the name up in`,
      "pass the workspace's UUID, or --org <slug> to find it by name (`oxyc api {org}/workspaces`)"
    );
  }
  const path = normalizePath(`orgs/${encodeURIComponent(await resolveOrgId(ctx, org))}/workspaces`);
  const rows = (await getJson(ctx, path)) as { id?: string; name?: string }[];
  const matches = (Array.isArray(rows) ? rows : []).flatMap((r) =>
    typeof r.id === "string" && r.name?.toLowerCase() === given.toLowerCase() ? [r.id] : []
  );
  const only = matches[0];
  if (matches.length === 1 && only) return only.toLowerCase();
  throw new CliError(
    matches.length === 0
      ? `no workspace named ${JSON.stringify(arg)} in ${org}`
      : `${matches.length} workspaces in ${org} are named ${JSON.stringify(arg)}`,
    {
      code: matches.length === 0 ? ExitCode.NOT_FOUND : ExitCode.USAGE,
      hint: "use the workspace's id: `oxyc api {org}/workspaces --jq '.[] | [.id, .name]'`"
    }
  );
}

const text = (v: unknown): string | undefined =>
  typeof v === "string" && v.trim() ? v.trim() : undefined;

/** Read the three repository fields off `/meta`. */
export async function fetchRepoFacts(ctx: Context, workspaceId: string): Promise<RepoFacts> {
  const meta = (await getJson(ctx, `/api/${encodeURIComponent(workspaceId)}/meta`)) as Record<
    string,
    unknown
  >;
  return {
    workspaceId,
    name: text(meta.name) ?? workspaceId,
    remoteUrl: text(meta.git_remote_url),
    defaultBranch: text(meta.default_branch),
    repoSubdir: text(meta.repo_subdir)
  };
}

function noRepository(facts: RepoFacts): CliError {
  return new CliError(
    `workspace ${facts.name} has no repository; connect one in the workspace's Settings`,
    { code: ExitCode.NOT_FOUND }
  );
}

/** The three closing lines, in order: where it is, how it ships, how to preview. */
export function closingLines(path: string, branch: string, workspaceId: string): string[] {
  return [
    path,
    `push to ${branch} to compile and deploy`,
    `preview a branch first (staff): oxyc preview create <branch> --workspace ${workspaceId} --wait`
  ];
}

/** Clone, then point at (and optionally open) the workspace folder. */
export async function runWorkspaceClone(
  ctx: Context,
  workspaceArg: string,
  flags: CloneFlags,
  deps: CloneDeps = realDeps()
): Promise<void> {
  const choice = parseOpen(flags.open);
  const id = await resolveWorkspaceId(ctx, workspaceArg);
  const facts = await fetchRepoFacts(ctx, id);
  if (!facts.remoteUrl) throw noRepository(facts);

  const branch = flags.branch ?? facts.defaultBranch;
  const repoDir = resolve(flags.dir ?? repoNameFromUrl(facts.remoteUrl));
  const target = workspacePath(repoDir, facts.repoSubdir);
  requireTargetDir(deps, repoDir);

  const args = cloneArgs(facts.remoteUrl, repoDir, branch);
  log.info(`cloning ${redactUrl(facts.remoteUrl)}${branch ? ` (${branch})` : ""} into ${repoDir}`);
  log.hint(`git ${args.map((a) => (a === facts.remoteUrl ? redactUrl(a) : a)).join(" ")}`);
  const status = deps.git(args);
  if (status !== 0) {
    throw new CliError(`git clone failed (exit ${status})`, {
      code: ExitCode.REQUEST,
      hint: "git's own message is above; the clone uses your git and its credentials, so check you can reach the repository with `git ls-remote`"
    });
  }

  if (facts.repoSubdir && !deps.isDir(target)) {
    log.warn(`${facts.repoSubdir}/ is not in the clone; opening the repository root instead`);
  }
  const shown = deps.isDir(target) ? target : repoDir;
  openEditor(choice, shown, deps);
  for (const line of closingLines(shown, facts.defaultBranch ?? "the default branch", id)) {
    process.stdout.write(`${line}\n`);
  }
}

function openEditor(choice: OpenChoice | undefined, path: string, deps: CloneDeps): void {
  const plan = planOpen(choice, deps);
  if (plan.command === "none") return;
  if (plan.launch) {
    deps.launch(plan.command, [path]);
    log.info(`opened in ${plan.command}`);
    return;
  }
  log.hint(`open it yourself: ${out.dim(`${plan.command} ${path}`)}`);
}
