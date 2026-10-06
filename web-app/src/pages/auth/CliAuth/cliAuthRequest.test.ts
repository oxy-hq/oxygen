import { describe, expect, it } from "vitest";
import {
  codeCallbackUrl,
  type LegacyRequest,
  loginUrl,
  type PkceRequest,
  parseCliAuthRequest,
  returnToUrl,
  tokenCallbackUrl
} from "./cliAuthRequest";

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

const parse = (query: string) => parseCliAuthRequest(new URLSearchParams(query));

describe("parseCliAuthRequest", () => {
  it("runs the legacy handoff for an older oxyc that sends no code_challenge", () => {
    expect(parse("port=53124&state=abc")).toEqual({ kind: "legacy", port: "53124", state: "abc" });
  });

  it("runs PKCE as soon as a code_challenge is present", () => {
    expect(parse(`port=53124&state=abc&code_challenge=${CHALLENGE}&hostname=luong-mbp`)).toEqual({
      kind: "pkce",
      port: "53124",
      state: "abc",
      codeChallenge: CHALLENGE,
      hostname: "luong-mbp"
    });
  });

  it("never downgrades a broken PKCE request to the token handoff", () => {
    // Each of these asked for PKCE. Falling back to legacy would put the session token in a URL.
    for (const query of [
      "port=1&state=s&code_challenge=",
      "port=1&state=s&code_challenge=has spaces&hostname=h",
      "port=1&state=s&code_challenge=<script>&hostname=h",
      // Not 43 characters, so not an S256 challenge: the server would answer 400.
      `port=1&state=s&code_challenge=${CHALLENGE.slice(1)}&hostname=h`,
      `port=1&state=s&code_challenge=${CHALLENGE}A&hostname=h`,
      // A control character in the hostname is refused by the server too.
      `port=1&state=s&code_challenge=${CHALLENGE}&hostname=h%07x`,
      `port=1&state=s&code_challenge=${CHALLENGE}`,
      `port=1&state=s&code_challenge=${CHALLENGE}&hostname=%20%20`,
      `port=1&state=s&code_challenge=${CHALLENGE}&hostname=${"h".repeat(256)}`
    ]) {
      expect(parse(query).kind).toBe("invalid");
    }
  });

  it("tolerates base64 padding on the challenge", () => {
    expect(parse(`port=1&state=s&code_challenge=${CHALLENGE}%3D&hostname=h`).kind).toBe("pkce");
  });

  it("refuses a missing or non-numeric port, or a missing state, on either flow", () => {
    for (const query of [
      "state=abc",
      "port=53124",
      "port=evil.example.com&state=abc",
      "port=80/x&state=abc",
      `port=nope&state=abc&code_challenge=${CHALLENGE}&hostname=h`
    ]) {
      expect(parse(query)).toEqual({
        kind: "invalid",
        reason: "Invalid CLI login request (missing or malformed port/state)."
      });
    }
  });
});

describe("the loopback handoff", () => {
  const pkce: PkceRequest = {
    kind: "pkce",
    port: "53124",
    state: "a b&c",
    codeChallenge: CHALLENGE,
    hostname: "luong mbp"
  };
  const legacy: LegacyRequest = { kind: "legacy", port: "53124", state: "a b&c" };

  it("sends PKCE a code, and no credential", () => {
    const url = codeCallbackUrl(pkce, "one/time+code");
    expect(url).toBe("http://127.0.0.1:53124/callback?code=one%2Ftime%2Bcode&state=a%20b%26c");
    expect(url).not.toContain("token=");
  });

  it("sends the legacy flow the session token, as it always has", () => {
    expect(tokenCallbackUrl(legacy, "jwt.value")).toBe(
      "http://127.0.0.1:53124/callback?token=jwt.value&state=a%20b%26c"
    );
  });

  it("only ever targets the local loopback", () => {
    expect(new URL(codeCallbackUrl(pkce, "c")).hostname).toBe("127.0.0.1");
    expect(new URL(tokenCallbackUrl(legacy, "t")).hostname).toBe("127.0.0.1");
  });
});

describe("returning after sign-in", () => {
  const origin = "https://app.oxy.tech";

  it("keeps the legacy return URL exactly as it was", () => {
    expect(returnToUrl(origin, { kind: "legacy", port: "53124", state: "a b" })).toBe(
      "https://app.oxy.tech/cli-auth?port=53124&state=a%20b"
    );
  });

  it("carries the PKCE params through login, so the flow resumes as PKCE", () => {
    const request = parse(`port=53124&state=abc&code_challenge=${CHALLENGE}&hostname=luong-mbp`);
    if (request.kind !== "pkce") throw new Error("expected a PKCE request");
    const returnTo = returnToUrl(origin, request);
    // Round-trip: what comes back from login parses to the same request.
    expect(parseCliAuthRequest(new URL(returnTo).searchParams)).toEqual(request);
    expect(loginUrl(returnTo)).toBe(`/login?return_to=${encodeURIComponent(returnTo)}`);
  });
});
