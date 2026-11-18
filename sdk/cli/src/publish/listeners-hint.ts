/**
 * The hint for a ship the server refused over event listeners.
 *
 * A promote answers `409` with `code: "event_listeners_broken"`; a publish
 * that breaks a listener answers plain text whose sentence says "pass
 * --break-listeners". The code wins when present; the text is the fallback.
 */

import { CliError } from "../util/errors.js";

export const EVENT_LISTENERS_BROKEN = "event_listeners_broken";

const HINT =
  "this build breaks a production event listener — fix the build to keep the event, or re-run with --break-listeners to ship anyway (audited)";

/** The hint when the refusal is about listeners, else `undefined`. */
export function listenersHint(code: string | undefined, message: string): string | undefined {
  if (code === EVENT_LISTENERS_BROKEN) return HINT;
  if (code === undefined && /--break-listeners/.test(message)) return HINT;
  return undefined;
}

/** `cause` with the listeners hint added when it is that refusal; any other error as it was. */
export function withListenersHint(cause: unknown): unknown {
  if (!(cause instanceof CliError) || cause.hint !== undefined) return cause;
  const hint = listenersHint(
    cause.serverCode,
    `${cause.serverMessage ?? ""} ${cause.detail ?? ""}`
  );
  if (hint === undefined) return cause;
  return new CliError(cause.message, {
    code: cause.code,
    detail: cause.detail,
    hint,
    remedy: cause.remedy,
    serverCode: cause.serverCode,
    serverMessage: cause.serverMessage
  });
}
