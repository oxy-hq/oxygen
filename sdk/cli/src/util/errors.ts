/**
 * Failure, as something the caller can branch on.
 *
 * The audience is half machine, and an agent decides what to do next *solely*
 * from the exit code — so "printed an error and exited 0" is the one failure
 * mode this file exists to make unrepresentable. Every error path here carries
 * a code, and `main.ts` is the only place that calls `process.exit`.
 */

/**
 * The exit-code contract. Stable, documented in `oxyc help exit-codes`, and
 * pinned by a test: a caller that branches on these is entitled to.
 *
 * 0/1/2 are the POSIX-conventional trio. The rest are ours, and each exists
 * because the *response* to it differs: an agent that gets AUTH re-runs
 * `oxyc login`, one that gets NOT_FOUND stops, one that gets UNAVAILABLE
 * retries. Collapsing them into 1 would make all three indistinguishable.
 */
export const ExitCode = {
  OK: 0,
  /** Something went wrong and there is nothing more specific to say. */
  FAILURE: 1,
  /** The invocation itself was wrong: unknown flag, missing argument, bad value. */
  USAGE: 2,
  /** No usable credential for this target. `oxyc login` is the fix. */
  AUTH: 4,
  /** The server answered 404, or a named customer/route/app does not exist. */
  NOT_FOUND: 5,
  /** The server answered 4xx other than 401/403/404 — the request was malformed. */
  REQUEST: 6,
  /** The server answered 5xx, or the network failed. Retryable. */
  UNAVAILABLE: 7,
  /** A refusal: the operation would have destroyed or overwritten something. */
  REFUSED: 8,
  /** `oxyc checks run`: a check ran and failed or timed out. */
  CHECK_FAILED: 9
} as const;

export type ExitCodeValue = (typeof ExitCode)[keyof typeof ExitCode];

/**
 * An error that already knows how it should end the process.
 *
 * THREE CHANNELS, and which one a string takes is a decision. `message` says
 * what went wrong. `hint` elaborates it — where to look, what the failure is
 * not, a command that helps. `remedy` is the one line about what to DO, and is
 * rendered set apart from the rest rather than in the run of elaborations.
 * Keeping them separate is what lets the renderer style them differently and
 * what stops any of them being buried mid-sentence where a reader skimming
 * stderr will miss it.
 *
 * NOT YET APPLIED EVERYWHERE. About seventy `hint:` values predate `remedy`,
 * and a fair number of them — `gh auth login`, `brew install jq` — are remedies
 * by this definition. Moved so far: the three sites that shipped ONE SENTENCE
 * through two channels, plus the one remedy left behind in a file being edited
 * for that reason — a leftover is least defensible in a file already open. The
 * rest is a sweep, deliberately not folded into the commit that introduced the
 * field.
 *
 * It is NOT UNIFORMLY a call-site edit. Three groups:
 *
 * - Sites building `new CliError` directly can move by renaming the field.
 *   `authError` is the obvious first one — it is the canonical remedy
 *   (`oxyc login --env …`) and the most-hit error in the tool, and nothing
 *   stands between it and the field.
 * - Sites going through `refusal` can now pass one: it takes an OPTIONS OBJECT
 *   rather than a fourth positional string, because four same-typed optionals
 *   in a row let a `detail` and a `remedy` be swapped with no compile error,
 *   and these two fields exist precisely to render differently.
 * - Sites going through `usageError` cannot yet. It is deliberately NOT widened
 *   on spec: of its ten callers four pass a hint in position two, and adding an
 *   optional third invites `usageError(msg, "run X")` to mean a remedy and
 *   render as one more `→`. The risk is the future caller, not the count. It
 *   gets an options object when one needs the field.
 */
export class CliError extends Error {
  readonly code: ExitCodeValue;
  readonly hint?: string;
  /** Extra lines printed verbatim under the message — a response body, a diff. */
  readonly detail?: string;

  /**
   * What to do about it, rendered set apart from the message.
   *
   * SEPARATE FROM `hint`, which shares one marker with path lists and other
   * elaborations. Without this the error path was the one consumer a remedy
   * could not reach in its own voice: `hint` renders through `log.hint`, so a
   * remedy handed to it printed as `→ chmod …` — an instruction in the run of
   * elaborations, which is what `log.remedy` exists to prevent.
   */
  readonly remedy?: string;

  constructor(
    message: string,
    opts: { code?: ExitCodeValue; hint?: string; detail?: string; remedy?: string } = {}
  ) {
    super(message);
    this.name = "CliError";
    this.code = opts.code ?? ExitCode.FAILURE;
    this.hint = opts.hint;
    this.detail = opts.detail;
    this.remedy = opts.remedy;
  }
}

/** The invocation was wrong. Distinct from a request that failed. */
export function usageError(message: string, hint?: string): CliError {
  return new CliError(message, { code: ExitCode.USAGE, hint });
}

/**
 * No credential for `target`.
 *
 * The hint names the exact command including the env, because the common case
 * is being logged into production and calling dev — where "run oxyc login"
 * without the `--env` sends you to log in again to the host you already have.
 */
export function authError(target: string, env: string, tokenEnv: string): CliError {
  return new CliError(`not authenticated for ${target}`, {
    code: ExitCode.AUTH,
    hint: `oxyc login --env ${env}   (or set ${tokenEnv})`
  });
}

/**
 * A refusal — the operation was understood and declined because doing it would
 * have cost something unrecoverable. Never used for "it failed"; only for
 * "it would have worked, and that is the problem".
 */
export function refusal(
  message: string,
  // `code?: never` REJECTS one, rather than the spread order silently winning.
  // Excess-property checking is a literal-only rule, so a variable carrying a
  // `code` is assignable to a parameter that merely omits the field — ordering
  // then decides who wins, and the caller gets no signal that what they asked
  // for did not happen. As a property-type mismatch it is a compile error even
  // for a variable.
  //
  // THE ORDER STILL MATTERS — that is the runtime half. The type stops a typed
  // caller; `{ ...opts, code }` below stops an `as any`, an `@ts-expect-error`,
  // and the day a sweep widens this signature and drops the `never`. A
  // `refusal()` that exits 0 is "printed an error and exited 0", which this
  // file's header names as the one thing it exists to make unrepresentable.
  opts: { hint?: string; detail?: string; remedy?: string; code?: never } = {}
): CliError {
  return new CliError(message, { ...opts, code: ExitCode.REFUSED });
}

/** Map an HTTP status onto the exit-code contract above. */
export function exitCodeForStatus(status: number): ExitCodeValue {
  if (status === 401 || status === 403) return ExitCode.AUTH;
  if (status === 404) return ExitCode.NOT_FOUND;
  if (status >= 500) return ExitCode.UNAVAILABLE;
  if (status >= 400) return ExitCode.REQUEST;
  return ExitCode.FAILURE;
}
