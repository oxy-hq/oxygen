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
 * What to tell whoever opened the link. The server gives one answer for every
 * way a ticket can be bad, so the copy lists them rather than guessing which,
 * and names the one fix they all share.
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
        title: "Sign-in link didn't work",
        description:
          "This link is invalid, has already been used, or has expired. Links work once and last 5 minutes — mint a new one with `oxyc login-link`."
      };
    case "server":
      return {
        title: "Couldn't reach the server",
        description:
          "The server could not be reached or did not answer, so this sign-in link may still be good. Open it again in a moment, or mint a new one with `oxyc login-link`."
      };
  }
};
