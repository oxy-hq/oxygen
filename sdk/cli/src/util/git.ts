/**
 * The git questions this tool asks, in one place.
 *
 * All read-only except `clone`. Nothing here commits, branches or pushes —
 * that is a deliberate boundary inherited from the bash tooling: every command
 * writes files into a working tree and stops, and taking that through a branch
 * and a pull request stays a human's job.
 */

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

import { CliError, ExitCode } from "../util/errors.js";

function git(args: string[], cwd?: string): { stdout: string; stderr: string; status: number } {
  const result = spawnSync("git", args, { cwd, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
  if (result.error) {
    throw new CliError(`git failed to start: ${result.error.message}`, {
      code: ExitCode.FAILURE,
      hint: "install git (xcode-select --install, or brew install git)"
    });
  }
  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status ?? 1
  };
}

/** Is `dir` inside a git working tree? */
export function isRepo(dir: string): boolean {
  return git(["rev-parse", "--is-inside-work-tree"], dir).status === 0;
}

/**
 * The repo ROOT for `dir`.
 *
 * Walks up to the real `.git`, which matters for the subdirectory-workspace
 * shape: a command run from `pokehouse-oxy/oxy/` must resolve the repo, not
 * the workspace.
 */
export function repoRoot(dir: string): string | undefined {
  const result = git(["rev-parse", "--show-toplevel"], dir);
  if (result.status !== 0) return undefined;
  return result.stdout.trim() || undefined;
}

/**
 * `<owner>/<name>` from a remote URL.
 *
 * DISCOVERY IS BY REMOTE, NOT BY PATH, and that is load-bearing rather than
 * tidy: on a real machine the checkouts sit under directory names that match
 * neither the GitHub org nor, in one case, the repo name. A
 * `<root>/<org>/<name>` convention finds none of them and reports four repos
 * as missing while all four are on disk.
 */
export function slugFromRemote(dir: string, remote = "origin"): string | undefined {
  const result = git(["remote", "get-url", remote], dir);
  if (result.status !== 0) return undefined;
  return parseRemoteSlug(result.stdout.trim());
}

/** `<owner>/<name>` from an SSH or HTTPS GitHub remote. */
export function parseRemoteSlug(url: string): string | undefined {
  const cleaned = url.trim().replace(/\.git$/, "");
  // git@github.com:owner/name  |  ssh://git@github.com/owner/name
  const ssh = /^(?:ssh:\/\/)?[^@]+@[^:/]+[:/](.+)$/.exec(cleaned);
  if (ssh?.[1]) {
    const parts = ssh[1].split("/").filter(Boolean);
    if (parts.length >= 2) return parts.slice(-2).join("/");
  }
  // https://github.com/owner/name
  try {
    const parts = new URL(cleaned).pathname.split("/").filter(Boolean);
    if (parts.length >= 2) return parts.slice(-2).join("/");
  } catch {
    // Not a URL — fall through.
  }
  return undefined;
}

/** The checked-out branch, or `undefined` on a detached HEAD. */
export function currentBranch(dir: string): string | undefined {
  const result = git(["symbolic-ref", "--quiet", "--short", "HEAD"], dir);
  if (result.status !== 0) return undefined;
  return result.stdout.trim() || undefined;
}

/** The commit `HEAD` points at. */
export function headSha(dir: string): string | undefined {
  const result = git(["rev-parse", "HEAD"], dir);
  return result.status === 0 ? result.stdout.trim() : undefined;
}

export interface WorkingTreeState {
  dirty: boolean;
  untracked: string[];
  modified: string[];
}

/**
 * What is uncommitted in `dir`.
 *
 * Untracked files are reported SEPARATELY from modified ones because the
 * commands that refuse on them refuse for different reasons: a modified file
 * is work in progress, an untracked one may be the only copy of something.
 */
export function workingTreeState(dir: string): WorkingTreeState {
  const result = git(["status", "--porcelain=v1", "--untracked-files=all"], dir);
  const untracked: string[] = [];
  const modified: string[] = [];
  for (const line of result.stdout.split("\n")) {
    if (!line.trim()) continue;
    const status = line.slice(0, 2);
    const path = line.slice(3);
    if (status === "??") untracked.push(path);
    else modified.push(path);
  }
  return { dirty: modified.length > 0, untracked, modified };
}

/**
 * Does this checkout have the commit `sha`?
 *
 * False for a commit that exists on the remote but was never fetched here —
 * this asks the local object store and nothing else.
 */
export function hasCommit(dir: string, sha: string): boolean {
  return git(["cat-file", "-e", `${sha}^{commit}`], dir).status === 0;
}

/** Is `ancestor` reachable from `descendant`? Both must be commits this checkout has. */
export function isAncestor(dir: string, ancestor: string, descendant = "HEAD"): boolean {
  return git(["merge-base", "--is-ancestor", ancestor, descendant], dir).status === 0;
}

export interface CommitLine {
  sha: string;
  /** Committer date, `YYYY-MM-DD`. */
  date: string;
  author: string;
  subject: string;
}

/** The commits in `range` (`a..b`) that touch `path`, newest first. */
export function commitsTouching(dir: string, range: string, path: string): CommitLine[] {
  const result = git(["log", "--format=%H%x1f%cs%x1f%an%x1f%s", range, "--", path], dir);
  if (result.status !== 0) {
    throw new CliError(`git log ${range} failed in ${dir}`, {
      code: ExitCode.FAILURE,
      detail: result.stderr.trim() || undefined
    });
  }
  return result.stdout
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const [sha = "", date = "", author = "", subject = ""] = line.split("\x1f");
      return { sha, date, author, subject };
    });
}

/** Tracked files matching `pathspec`, relative to the repo root `dir`. */
export function trackedFiles(dir: string, pathspec: string[]): string[] {
  const result = git(["ls-files", "-z", "--", ...pathspec], dir);
  if (result.status !== 0) return [];
  return result.stdout.split("\0").filter(Boolean);
}

/**
 * The uncommitted paths under `path`: modified, staged, or untracked.
 *
 * `--no-optional-locks` because a plain `git status` refreshes the index as a
 * side effect, and the callers of this promise not to write to the checkout.
 *
 * Throws when `git status` fails. An empty list means "nothing uncommitted",
 * and a status that could not be read is not that.
 */
export function uncommittedIn(dir: string, path: string): string[] {
  const result = git(
    ["--no-optional-locks", "status", "--porcelain=v1", "--untracked-files=all", "--", path],
    dir
  );
  if (result.status !== 0) {
    throw new CliError(`git status failed in ${dir}`, {
      code: ExitCode.FAILURE,
      detail: result.stderr.trim() || undefined
    });
  }
  return result.stdout
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => line.slice(3));
}

/** Clone `slug` into `dest`, preferring `gh` so private repos work unattended. */
export function clone(slug: string, dest: string, useGh: boolean): void {
  const [bin, args] = useGh
    ? (["gh", ["repo", "clone", slug, dest]] as const)
    : (["git", ["clone", `https://github.com/${slug}.git`, dest]] as const);
  const result = spawnSync(bin, [...args], { stdio: "inherit" });
  if (result.status !== 0) {
    throw new CliError(`could not clone ${slug} into ${dest}`, {
      code: ExitCode.FAILURE,
      hint: useGh ? undefined : "install gh for private repos: brew install gh && gh auth login"
    });
  }
}

/** Does `dir` look like a checkout at all? */
export function looksLikeCheckout(dir: string): boolean {
  return existsSync(join(dir, ".git"));
}
