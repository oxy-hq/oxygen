/**
 * PKCE (RFC 7636), S256 only.
 *
 * `oxyc login` sends the CHALLENGE to the browser and keeps the VERIFIER in
 * this process. The page hands back a one-time code, and only the holder of
 * the verifier can trade it for a token — so nothing that can read the
 * loopback URL (browser history, a proxy log, another local process watching
 * the port) ends up holding a credential.
 */

import { createHash, randomBytes } from "node:crypto";

export interface Pkce {
  /** Stays in this process; sent only to `POST /api/auth/cli/exchange`. */
  verifier: string;
  /** `base64url(sha256(verifier))`, unpadded — what the browser carries. */
  challenge: string;
}

/** The S256 challenge for a verifier. Exported so a test can pin the transform. */
export function challengeFor(verifier: string): string {
  return createHash("sha256").update(verifier, "ascii").digest("base64url");
}

/**
 * A fresh pair. 32 random bytes encode to 43 base64url characters — the
 * minimum the RFC allows, and every one of them from its unreserved set.
 */
export function createPkce(): Pkce {
  const verifier = randomBytes(32).toString("base64url");
  return { verifier, challenge: challengeFor(verifier) };
}
