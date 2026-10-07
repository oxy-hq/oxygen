/**
 * A workspace whose git working copy is on a detached HEAD — a CI
 * `pull_request` checkout, `git checkout <sha>`, `git worktree add --detach`.
 *
 * There is no branch. The server still reports a value in `active_branch`
 * (the label `HEAD@<sha>`) and takes it back as `?branch=`, where it means
 * "the working copy as it is", so reads and saves work. The label's shape is
 * the server's business: it also sends `detached_head` (the short sha), and
 * that field — never a parse of the label — is how the frontend knows.
 *
 * Operations that need a real branch (commit/push, pull, fetch, restore,
 * switching to the label) are refused with `409 {"code":"detached_head"}` and
 * a message that says what to do.
 */
const DETACHED_HEAD_CODE = "detached_head";

const FALLBACK_MESSAGE =
  "This workspace is on a detached HEAD; switch to or create a branch first.";

interface DetachedWorkspace {
  active_branch?: { name: string } | null;
  detached_head?: string | null;
}

/**
 * The short sha when `branch` IS the workspace's detached working copy, else
 * `null` — including when the workspace is detached but the IDE is looking at
 * one of its real branches (each lives in its own worktree).
 */
export function detachedHeadFor(
  workspace: DetachedWorkspace | null | undefined,
  branch: string | null | undefined
): string | null {
  const sha = workspace?.detached_head;
  if (!sha || !branch) return null;
  return workspace?.active_branch?.name === branch ? sha : null;
}

/** What the branch control reads instead of a branch name. */
export function detachedHeadLabel(sha: string): string {
  return `Detached at ${sha}`;
}

/** The message of a `409 detached_head` refusal, or `null` for any other response. */
export function readDetachedHeadBody(status: number | undefined, body: unknown): string | null {
  if (status !== 409) return null;
  const data = body as { code?: unknown; message?: unknown } | null | undefined;
  if (data?.code !== DETACHED_HEAD_CODE) return null;
  return typeof data.message === "string" && data.message.trim() ? data.message : FALLBACK_MESSAGE;
}
