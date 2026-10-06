/**
 * PKCE is a protocol shared with the server, which recomputes the challenge
 * from the verifier it is handed. A transform that is off by an encoding — hex
 * for base64url, padded for unpadded — produces a login that always fails at
 * the exchange, so the transform is pinned against the RFC's own vector.
 */

import { describe, expect, it } from "vitest";
import { challengeFor, createPkce } from "./pkce.js";

describe("challengeFor", () => {
  it("matches the RFC 7636 appendix B test vector", () => {
    expect(challengeFor("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk")).toBe(
      "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
  });
});

describe("createPkce", () => {
  it("makes a verifier of the RFC's length and alphabet, and its S256 challenge", () => {
    const { verifier, challenge } = createPkce();
    // 43–128 characters from the unreserved set; 32 random bytes are 43.
    expect(verifier).toMatch(/^[A-Za-z0-9\-._~]{43,128}$/);
    // base64url of a SHA-256: 43 characters, never padded.
    expect(challenge).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(challenge).toBe(challengeFor(verifier));
    expect(challenge).not.toBe(verifier);
  });

  it("never repeats", () => {
    const seen = new Set(Array.from({ length: 50 }, () => createPkce().verifier));
    expect(seen.size).toBe(50);
  });
});
