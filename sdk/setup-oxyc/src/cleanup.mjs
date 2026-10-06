/**
 * The post step: revoke the token the main step minted.
 *
 * NEVER FAILS THE JOB. By the time this runs the job's real work is done, and
 * the token expires on its own in fifteen minutes whatever happens here — so a
 * revoke that could not be confirmed is a warning in the log, not a red run.
 * Revoking early is hygiene: it shrinks the window in which a token that
 * leaked into a log or an artifact still works from fifteen minutes to the
 * length of the job.
 */

import { getState, info, mask, warning } from "./io.mjs";
import { revoke } from "./oxy.mjs";

/** @typedef {import("./io.mjs").Io} Io */

export async function cleanup(/** @type {Io} */ io) {
  const token = getState(io, "token");
  const host = getState(io, "host");
  if (!token || !host) {
    info(io, "no token was minted by this job — nothing to revoke");
    return;
  }
  // Masks set in the main step still apply; this one costs nothing and does
  // not depend on that.
  mask(io, token);

  const outcome = await revoke(io, host, token);
  switch (outcome) {
    case "revoked":
      info(io, `revoked the token minted for this job on ${host}`);
      return;
    case "already_invalid":
      info(io, `${host} no longer accepts the token — it was already revoked or has expired`);
      return;
    case "unsupported":
      warning(
        io,
        `${host} has no route to revoke a token (DELETE /api/auth/token). It expires on its own, fifteen minutes after it was minted.`
      );
      return;
    default:
      warning(
        io,
        `could not confirm the token was revoked on ${host}. It expires on its own, fifteen minutes after it was minted.`
      );
  }
}
