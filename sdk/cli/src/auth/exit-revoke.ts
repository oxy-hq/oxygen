/**
 * Tokens this process minted, and must not leave behind.
 *
 * A token exchanged from a GitHub OIDC token lives fifteen minutes whatever
 * happens, so this is hygiene rather than a guarantee: a one-shot command
 * revokes what it minted on the way out, and the window in which a leaked CI
 * log line is a working credential shrinks from fifteen minutes to the length
 * of the command.
 *
 * BEST-EFFORT, IN BOTH DIRECTIONS. A revoke that fails never fails the
 * command — `revokeCallingToken` does not throw, and nothing here inspects its
 * answer. And a process killed outright (`SIGKILL`, a runner pulled mid-job)
 * revokes nothing; the fifteen minutes are the backstop.
 */

import { revokeCallingToken } from "./token-api.js";

/** token → the deployment that minted it. */
const pending = new Map<string, string>();
let signalsHooked = false;

/** How long cleanup may hold up an exit. A slow revoke is not worth a hung job. */
const EXIT_REVOKE_TIMEOUT_MS = 5_000;
/** Shorter under a signal: the runner is already counting down to SIGKILL. */
const SIGNAL_REVOKE_TIMEOUT_MS = 3_000;

/**
 * Revoke `token` when the process ends.
 *
 * The signal handlers are installed here, on the first token, rather than at
 * startup: a process that minted nothing keeps node's default signal
 * behaviour, untouched.
 */
export function revokeOnExit(target: string, token: string): void {
  pending.set(token, target);
  if (signalsHooked) return;
  signalsHooked = true;
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    // `once`, then re-raise: with the listener gone the default action runs,
    // so the process still dies BY the signal and its parent sees that.
    process.once(signal, () => {
      void runExitRevokes(SIGNAL_REVOKE_TIMEOUT_MS).finally(() => {
        process.kill(process.pid, signal);
      });
    });
  }
}

/**
 * Leave `token` alive past the end of the process.
 *
 * For `oxyc token`, whose whole output IS the token: revoking it on exit would
 * hand the caller a string that died before they could use it.
 */
export function keepPastExit(token: string): void {
  pending.delete(token);
}

/** Tokens still queued for revocation. For tests. */
export function pendingRevokes(): string[] {
  return [...pending.keys()];
}

/** Revoke everything queued. Never throws; safe to call more than once. */
export async function runExitRevokes(timeoutMs = EXIT_REVOKE_TIMEOUT_MS): Promise<void> {
  const queued = [...pending.entries()];
  pending.clear();
  await Promise.all(
    queued.map(([token, target]) => revokeCallingToken(target, token, timeoutMs).catch(() => {}))
  );
}
