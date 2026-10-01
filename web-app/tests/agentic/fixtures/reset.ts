// Setup commands available to flows. Fixtures may only:
//   - put a known state into the workspace under test at known-fixed paths
//     inside it (and only via `reset_test_file` / `restore_demo_file:`)
//   - navigate the browser (`goto:`)
//
// Fixtures must NEVER seed, drop, or otherwise mutate any external
// system — database, warehouse, port-forward, shared service. Any new
// command that would touch a non-local endpoint requires explicit
// design review.
//
// "The workspace under test" is not always a directory on this machine. With
// `--local` it is `demo_project/` right here, and these commands write to it
// directly. Against a cloud or fleet deployment the workspace is a git working
// copy owned by the ide node — writing `demo_project/` there changes a
// directory nothing under test will ever read, and the flow then drives a file
// the deployment does not have. `test.sql` is the sharp case: it is gitignored
// (demo_project/.gitignore:19), so a seeded workspace does not carry it at all,
// and `ide-save` / `ide-compile-error` open a file that was never there.
//
// So when `OXY_FIXTURE_WORKSPACE` names a workspace, these two commands write
// through the IDE's own save-file endpoint instead — the same call the editor
// makes when a user hits save. That is a product API, not a back channel: it
// respects auth, branch, and the IdeOnly proxy, and it is the only form that
// works no matter where the files live. Unset, nothing changes.

import { execFileSync } from "node:child_process";
import { lstatSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
const DEMO_PROJECT_DIR = resolve(REPO_ROOT, "demo_project");
const SCRATCH_SQL = resolve(DEMO_PROJECT_DIR, "test.sql");

export interface SetupContext {
  goto: (url: string) => Promise<void>;
}

/**
 * Workspace to write fixtures into over HTTP. Unset (the `--local` case) means
 * the workspace is `demo_project/` on this machine and the writes stay local.
 */
function remoteTarget(): {
  base: string;
  workspace: string;
  branch: string;
  token?: string;
} | null {
  const workspace = process.env.OXY_FIXTURE_WORKSPACE;
  if (!workspace) return null;
  return {
    base: process.env.OXY_BASE_URL ?? "http://localhost:3000",
    workspace,
    branch: process.env.OXY_FIXTURE_BRANCH ?? "main",
    token: process.env.OXY_SESSION_TOKEN
  };
}

/**
 * Write one workspace file through `POST /api/<ws>/files/<b64>` — the editor's
 * save call. `new-file` first because the path may not exist yet (again:
 * `test.sql` is gitignored, so a fresh workspace has no copy); its failure is
 * ignored because "already exists" is the expected answer on every rerun. The
 * save itself must succeed, and a failure throws so a broken fixture surfaces
 * as a setup error rather than as a mystery timeout six steps later.
 */
async function writeRemoteFile(rel: string, content: string): Promise<void> {
  const target = remoteTarget();
  if (!target) throw new Error("writeRemoteFile called without OXY_FIXTURE_WORKSPACE");
  if (!rel || rel.includes("..") || rel.startsWith("/")) {
    throw new Error(`fixture refuses to write outside the workspace: ${JSON.stringify(rel)}`);
  }
  // The 2026-05-06 incident was not a fixture that meant to touch production —
  // it was a fixture whose DEFAULTS happened to match a port-forward to it
  // (README.md:13). The structural lesson was to remove fixture code that even
  // CAN reach a real system, so a fixture that writes over HTTP has to make that
  // shape impossible rather than unlikely. `OXY_BASE_URL` defaults to localhost
  // but is free-form, so a workspace id set against a base URL pointing anywhere
  // real would overwrite files in that workspace. Pin it to loopback: a
  // deliberate remote target has to say so in a variable whose name is what it
  // does.
  const host = new URL(target.base).hostname;
  const loopback = host === "localhost" || host === "127.0.0.1" || host === "::1";
  if (!loopback && process.env.OXY_FIXTURE_ALLOW_REMOTE !== "1") {
    throw new Error(
      `fixture refuses to write to a non-loopback deployment (${target.base}). ` +
        "Set OXY_FIXTURE_ALLOW_REMOTE=1 only if that host is a disposable test deployment."
    );
  }

  const b64 = Buffer.from(rel, "utf-8").toString("base64");
  const qs = `?branch=${encodeURIComponent(target.branch)}`;
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (target.token) headers.Cookie = `oxy_session=${target.token}`;
  const url = `${target.base}/api/${target.workspace}/files/${encodeURIComponent(b64)}`;

  await fetch(`${url}/new-file${qs}`, { method: "POST", headers }).catch(() => undefined);
  const res = await fetch(`${url}${qs}`, {
    method: "POST",
    headers,
    body: JSON.stringify({ data: content })
  });
  if (!res.ok) {
    throw new Error(
      `fixture save of ${rel} failed: ${res.status} ${await res.text().catch(() => "")}`
    );
  }
}

export async function runSetup(commands: string[], ctx: SetupContext): Promise<void> {
  const remote = remoteTarget();
  for (const cmd of commands) {
    if (cmd === "reset_test_file") {
      if (remote) await writeRemoteFile("test.sql", "");
      else resetTestFile();
    } else if (cmd.startsWith("restore_demo_file:")) {
      const rel = cmd.slice("restore_demo_file:".length);
      // The committed content is still read from this checkout's HEAD — that is
      // the canonical copy either way, and reading it here keeps the remote
      // branch free of any assumption about what the deployment was seeded from.
      if (remote) await writeRemoteFile(rel, committedDemoFile(rel));
      else restoreDemoFile(rel);
    } else if (cmd.startsWith("goto:")) {
      await ctx.goto(cmd.slice("goto:".length));
    } else {
      throw new Error(`unknown setup command: ${cmd}`);
    }
  }
}

// Restore a `demo_project/<rel>` file to its committed-in-HEAD content
// so reruns of a flow that mutates the file (e.g. the builder editing
// insights.app.yml) start from the same canonical state. Reads from
// HEAD via `git show` so a developer's staged changes elsewhere are
// untouched.
function restoreDemoFile(rel: string): void {
  if (!rel || rel.includes("..")) {
    throw new Error(`restore_demo_file: invalid relative path ${JSON.stringify(rel)}`);
  }
  const target = resolve(DEMO_PROJECT_DIR, rel);
  const realRepoRoot = realpathSync(REPO_ROOT);
  // Same parent-walk safety check as resetTestFile: refuse symlinks and
  // any resolution that escapes the repo root.
  let probe = target;
  while (probe !== dirname(probe)) {
    try {
      const stat = lstatSync(probe);
      if (stat.isSymbolicLink()) {
        throw new Error(`restore_demo_file refuses to operate through a symbolic link: ${probe}`);
      }
      const real = realpathSync(probe);
      if (!real.startsWith(realRepoRoot)) {
        throw new Error(
          `restore_demo_file refuses to write outside the repo (resolved ${probe} → ${real})`
        );
      }
      break;
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code === "ENOENT") {
        probe = dirname(probe);
        continue;
      }
      throw err;
    }
  }
  writeFileSync(target, committedDemoFile(rel));
}

/**
 * The committed content of `demo_project/<rel>`, read from HEAD rather than the
 * working tree so a developer's staged or unstaged edits elsewhere do not leak
 * into a fixture. Shared by the local and remote write paths.
 */
function committedDemoFile(rel: string): string {
  if (!rel || rel.includes("..")) {
    throw new Error(`restore_demo_file: invalid relative path ${JSON.stringify(rel)}`);
  }
  const repoRel = relative(REPO_ROOT, resolve(DEMO_PROJECT_DIR, rel));
  return execFileSync("git", ["show", `HEAD:${repoRel}`], {
    cwd: REPO_ROOT,
    encoding: "utf-8",
    maxBuffer: 16 * 1024 * 1024
  });
}

// Wipe `demo_project/test.sql` so the ide-save flow starts from a clean
// state on every run. Sanity-checked to refuse anything other than the
// exact in-repo path — if `demo_project` were ever symlinked outside the
// repo (or the resolution is otherwise weird), the realpath check fails
// loudly rather than writing somewhere unexpected.
function resetTestFile(): void {
  const realRepoRoot = realpathSync(REPO_ROOT);
  // Walk parents of SCRATCH_SQL until we hit something that exists.
  // Validate that the resolved path lives inside the repo.
  let probe = SCRATCH_SQL;
  while (probe !== dirname(probe)) {
    try {
      const stat = lstatSync(probe);
      if (stat.isSymbolicLink()) {
        throw new Error(`reset_test_file refuses to operate through a symbolic link: ${probe}`);
      }
      const real = realpathSync(probe);
      if (!real.startsWith(realRepoRoot)) {
        throw new Error(
          `reset_test_file refuses to write outside the repo (resolved ${probe} → ${real})`
        );
      }
      break;
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code === "ENOENT") {
        probe = dirname(probe);
        continue;
      }
      throw err;
    }
  }
  writeFileSync(SCRATCH_SQL, "");
}
