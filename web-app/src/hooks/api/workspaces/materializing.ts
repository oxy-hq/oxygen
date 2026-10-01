/**
 * Retry + state derivation for a workspace whose working copy is not on disk
 * yet (ide pod restart / k8s rolling update).
 *
 * Kept apart from `useWorkspaces.ts` so it stays pure — importing the hook
 * module pulls in the Axios client, which touches `window` at load time.
 */

import { isWorkspaceMaterializingError } from "@/libs/utils/ideHealth";

/**
 * How many times to re-poll a materialising workspace before giving up. At the
 * backend's `Retry-After: 5` this covers ~2 minutes — long enough to ride out a
 * rolling update. Past it we stop polling AND say so (see
 * `materializingTimedOut`); a cap that only silenced the network would leave a
 * stuck volume spinning forever.
 */
const MATERIALIZING_MAX_RETRIES = 24;

/** React Query's own default, preserved for every other failure. */
const DEFAULT_RETRIES = 3;

/**
 * A workspace still materialising gets a long leash; everything else keeps
 * React Query's default count. Overriding `retry` replaces the default
 * wholesale, so real errors would silently drop to zero retries otherwise.
 */
export function shouldRetryWorkspaceQuery(failureCount: number, error: unknown): boolean {
  return isWorkspaceMaterializingError(error)
    ? failureCount < MATERIALIZING_MAX_RETRIES
    : failureCount < DEFAULT_RETRIES;
}

/** The subset of React Query state the materializing decision reads. */
export interface MaterializingInputs {
  isPending: boolean;
  isError: boolean;
  error: unknown;
  failureReason: unknown;
}

export interface MaterializingState {
  /** Still coming up, still retrying. Render a loading state. */
  isMaterializing: boolean;
  /**
   * Retries exhausted and it never came up. A real, terminal condition —
   * callers must surface it rather than spin, but it is NOT the generic load
   * failure (no toast-and-redirect; there is nowhere better to go).
   */
  materializingTimedOut: boolean;
}

/**
 * Split "still coming up" from "gave up waiting".
 *
 * These read DIFFERENT fields, which is the whole subtlety: while React Query
 * retries, the query stays PENDING and the interim error lives on
 * `failureReason` — `error` stays null until the query settles. Deriving both
 * from `error` (an earlier cut did) inverts them: the flag is false for the
 * entire retry window and then true forever after, pinning the shell on a
 * spinner for a workspace that is never coming back.
 */
export function deriveMaterializingState(q: MaterializingInputs): MaterializingState {
  return {
    isMaterializing: q.isPending && isWorkspaceMaterializingError(q.failureReason),
    materializingTimedOut: q.isError && isWorkspaceMaterializingError(q.error)
  };
}
