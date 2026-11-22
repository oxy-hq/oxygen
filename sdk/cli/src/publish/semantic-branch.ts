/**
 * `oxyc publish --semantic-branch <branch>`: compile a WORKSPACE branch into a
 * staging revision and return its id, which the publish then sends as
 * `semantic_revision_id` so the draft build's staging preview reads that
 * branch's semantic model. Live never does — see
 * `internal-docs/customer-apps-staging.md` (D4).
 *
 * Not `--branch`, which records the app SOURCE branch.
 */

import { withUserAgent } from "../api/user-agent.js";
import { bearerHeader } from "../auth/token-kind.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";

const REQUEST_TIMEOUT_MS = 30_000;
/** A compile of a large workspace takes minutes; past this, give up loudly. */
export const WAIT_TIMEOUT_MS = 10 * 60_000;
const POLL_MS = 1_000;

/** One per-file failure of a `failed` revision: `oxy_compile::FileFailure`. */
export interface CompileFailure {
  /** Workspace-relative, forward slashes. */
  path: string;
  /** `yaml` | `io` | `shape` | `duplicate` | `validation` | … (`FailureKind`). */
  kind: string;
  message: string;
}

/** The server's answer (`compile_request::CompileState`, the body of both `/compile/staging` routes). */
export interface StagingCompile {
  workspace_id?: string;
  git_sha: string;
  /** `stale`: a ready revision this server version won't reuse — re-POST to recompile. */
  status: "ready" | "compiling" | "pending" | "stale" | "failed";
  revision_id?: string | null;
  error?: string | null;
  /** Set when THIS call enqueued the compile. */
  task_id?: string | null;
  /** A `failed` revision's per-file failures; absent otherwise (and on an older server). */
  failures?: CompileFailure[];
  file_count_seen?: number;
  file_count_compiled?: number;
  file_count_failed?: number;
}

/** Whether the compile is still on its way: queued, compiling, or `stale` (ask again). */
export function inFlight(state: StagingCompile): boolean {
  return state.status === "pending" || state.status === "compiling" || state.status === "stale";
}

async function call(url: string, token: string, method: "GET" | "POST"): Promise<StagingCompile> {
  let response: Response;
  try {
    response = await fetch(url, {
      method,
      headers: withUserAgent(bearerHeader(token)),
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS)
    });
  } catch (cause) {
    throw new CliError(`${method} ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  const text = await response.text();
  if (!response.ok) {
    throw new CliError(`semantic branch compile failed (${response.status})`, {
      code: exitCodeForStatus(response.status),
      detail: text.slice(0, 2000)
    });
  }
  return JSON.parse(text) as StagingCompile;
}

/** The two requests a wait makes; each caller sends them its own way. */
export interface StagingCompileWire {
  /** `POST …/compile/staging?branch=<b>`. */
  post(): Promise<StagingCompile>;
  /** `GET …/compile/staging/status?git_sha=<sha>`. */
  status(gitSha: string): Promise<StagingCompile>;
}

/**
 * Poll from `first` (the POST's answer) until the compile is `ready` or
 * `failed`, and return that state — a failure is returned, not thrown, so each
 * caller decides what it means. `onTimeout` is called (and must throw) when the
 * deadline passes first.
 *
 * Shared by `oxyc publish --semantic-branch` and `oxyc compile --wait`, so the
 * `stale` handling below is written once.
 */
export async function waitForStagingCompile(
  first: StagingCompile,
  wire: StagingCompileWire,
  opts: {
    pollMs?: number;
    timeoutMs?: number;
    onWait?: (s: StagingCompile) => void;
    onTimeout: (s: StagingCompile) => never;
  }
): Promise<StagingCompile> {
  let state = first;
  const deadline = Date.now() + (opts.timeoutMs ?? WAIT_TIMEOUT_MS);
  // Re-POST a stale revision only if no compile of ours is queued. A compile we
  // enqueued has no revision row until a worker claims it (the queue polls
  // every ~10s), and until then `/status` answers `stale` for the old row — so
  // after our own enqueue, `stale` means "not claimed yet", and a re-POST
  // would only duplicate it. The one case left is a POST that joined a compile
  // another (older) node started, whose result this server won't reuse: then
  // we ask once. `task_id` on a POST's answer is what says "we enqueued".
  let enqueued = Boolean(state.task_id);
  while (inFlight(state)) {
    if (Date.now() > deadline) opts.onTimeout(state);
    opts.onWait?.(state);
    await new Promise((r) => setTimeout(r, opts.pollMs ?? POLL_MS));
    // Stale, and nothing of ours queued: ask for a fresh compile rather than
    // poll a revision nobody is recompiling until the timeout says it "did not
    // finish". At most once — `enqueued` holds after any re-POST.
    const repost: boolean = state.status === "stale" && !enqueued;
    state = repost ? await wire.post() : await wire.status(state.git_sha);
    enqueued ||= repost;
  }
  return state;
}

/**
 * Compile `branch` of workspace `project` into a staging revision (or reuse a
 * ready one of the same commit) and wait until it is ready.
 */
export async function compileSemanticBranch(
  target: string,
  token: string,
  project: string,
  branch: string,
  opts: { pollMs?: number; timeoutMs?: number; onWait?: (s: StagingCompile) => void } = {}
): Promise<string> {
  const base = `${target.replace(/\/+$/, "")}/api/${encodeURIComponent(project)}/compile/staging`;
  const wire: StagingCompileWire = {
    post: () => call(`${base}?branch=${encodeURIComponent(branch)}`, token, "POST"),
    status: (sha) => call(`${base}/status?git_sha=${encodeURIComponent(sha)}`, token, "GET")
  };
  const state = await waitForStagingCompile(await wire.post(), wire, {
    ...opts,
    onTimeout: () => {
      throw new CliError(`semantic branch ${branch} did not finish compiling in time`, {
        code: ExitCode.UNAVAILABLE,
        hint: "re-run the publish — a finished compile of the same commit is reused"
      });
    }
  });
  if (state.status !== "ready" || !state.revision_id) {
    throw new CliError(`semantic branch ${branch} failed to compile (${state.git_sha})`, {
      code: ExitCode.FAILURE,
      detail: state.error ?? undefined,
      hint: "fix the branch's YAML and push; the IDE's compile status shows the per-file errors"
    });
  }
  return state.revision_id;
}
