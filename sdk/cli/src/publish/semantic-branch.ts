/**
 * `oxyc publish --semantic-branch <branch>`: compile a WORKSPACE branch into a
 * staging revision and return its id, which the publish then sends as
 * `semantic_revision_id` so the draft build's staging preview reads that
 * branch's semantic model. Live never does — see
 * `internal-docs/customer-apps-staging.md` (D4).
 *
 * Not `--branch`, which records the app SOURCE branch.
 */

import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";

const REQUEST_TIMEOUT_MS = 30_000;
/** A compile of a large workspace takes minutes; past this, give up loudly. */
const WAIT_TIMEOUT_MS = 10 * 60_000;
const POLL_MS = 1_000;

/** The server's answer (`compile_staging::StagingCompileResponse`). */
export interface StagingCompile {
  git_sha: string;
  /** `stale`: a ready revision this server version won't reuse — re-POST to recompile. */
  status: "ready" | "compiling" | "pending" | "stale" | "failed";
  revision_id?: string | null;
  error?: string | null;
  /** Set when THIS call enqueued the compile. */
  task_id?: string | null;
}

async function call(url: string, token: string, method: "GET" | "POST"): Promise<StagingCompile> {
  let response: Response;
  try {
    response = await fetch(url, {
      method,
      headers: { authorization: `Bearer ${token}` },
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
  let state = await call(`${base}?branch=${encodeURIComponent(branch)}`, token, "POST");
  const deadline = Date.now() + (opts.timeoutMs ?? WAIT_TIMEOUT_MS);
  // Re-POST a stale revision only if no compile of ours is queued. A compile we
  // enqueued has no revision row until a worker claims it (the queue polls
  // every ~10s), and until then `/status` answers `stale` for the old row — so
  // after our own enqueue, `stale` means "not claimed yet", and a re-POST
  // would only duplicate it. The one case left is a POST that joined a compile
  // another (older) node started, whose result this server won't reuse: then
  // we ask once. `task_id` on a POST's answer is what says "we enqueued".
  let enqueued = Boolean(state.task_id);
  while (state.status === "pending" || state.status === "compiling" || state.status === "stale") {
    if (Date.now() > deadline) {
      throw new CliError(`semantic branch ${branch} did not finish compiling in time`, {
        code: ExitCode.UNAVAILABLE,
        hint: "re-run the publish — a finished compile of the same commit is reused"
      });
    }
    opts.onWait?.(state);
    await new Promise((r) => setTimeout(r, opts.pollMs ?? POLL_MS));
    // Stale, and nothing of ours queued: ask for a fresh compile rather than
    // poll a revision nobody is recompiling until the timeout says it "did not
    // finish". At most once — `enqueued` holds after any re-POST.
    const repost: boolean = state.status === "stale" && !enqueued;
    state = repost
      ? await call(`${base}?branch=${encodeURIComponent(branch)}`, token, "POST")
      : await call(`${base}/status?git_sha=${encodeURIComponent(state.git_sha)}`, token, "GET");
    enqueued ||= repost;
  }
  if (state.status !== "ready" || !state.revision_id) {
    throw new CliError(`semantic branch ${branch} failed to compile (${state.git_sha})`, {
      code: ExitCode.FAILURE,
      detail: state.error ?? undefined,
      hint: "fix the branch's YAML and push; the IDE's compile status shows the per-file errors"
    });
  }
  return state.revision_id;
}
