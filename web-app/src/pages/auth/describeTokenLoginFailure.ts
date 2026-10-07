/**
 * Which side a `/token-login` attempt failed on. `link` — the ticket is
 * missing, or the server refused it; `server` — the server never gave an
 * answer about the ticket at all.
 */
export type TokenLoginFailure = "link" | "server";

/**
 * The redeem endpoint has exactly one refusal: `400 invalid_ticket`, for a
 * ticket that is unknown, already used, expired, or whose token was revoked.
 * Anything else — no response, a 5xx, a proxy's 502 — says nothing about the
 * link, so it must not be reported as the link's fault.
 */
export const classifyTokenLoginFailure = (httpStatus: number | undefined): TokenLoginFailure =>
  httpStatus === 400 ? "link" : "server";

/**
 * What to tell whoever opened the link, in as few words as will do. The server
 * gives one answer for every way a ticket can be bad, so the copy states the
 * rule they all break rather than guessing which. The fix is the same either
 * way and is a command, so the page prints it as one ({@link NEW_LINK_COMMAND})
 * instead of burying it in a sentence.
 *
 * Lives beside the page rather than inside it, like `describeDevLoginFailure`,
 * so the copy can be pinned by a test that needs nothing but a string function.
 */
export const describeTokenLoginFailure = (
  failure: TokenLoginFailure
): { title: string; description: string } => {
  switch (failure) {
    case "link":
      return {
        title: "Link didn't work",
        description: "Links work once and expire in 5 minutes. Get a new one:"
      };
    case "server":
      return {
        title: "Couldn't reach the server",
        description: "The link may still be good. Open it again, or get a new one:"
      };
  }
};

/** What mints a fresh link. Shown under either failure. */
export const NEW_LINK_COMMAND = "oxyc login-link";
