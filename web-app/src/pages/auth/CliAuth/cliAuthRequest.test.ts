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

describe("parseCliAuthRequest, with a kind", () => {
  const PKCE = `port=53124&state=abc&code_challenge=${CHALLENGE}&hostname=luong-mbp`;

  it("reads a sandbox agent mint: the PKCE params, and what to mint as it was sent", () => {
    expect(
      parse(`${PKCE}&kind=sandbox_agent&apps=acme/store-ops,globex/pos&hours=8&name=refunds%20task`)
    ).toEqual({
      kind: "mint",
      port: "53124",
      state: "abc",
      codeChallenge: CHALLENGE,
      hostname: "luong-mbp",
      ask: { apps: ["acme/store-ops", "globex/pos"], hours: "8", name: "refunds task" }
    });
  });

  it("keeps each app once, and leaves out-of-range values for the page to show", () => {
    const request = parse(
      `${PKCE}&kind=sandbox_agent&apps=%20acme/a%20,,acme/a,acme/b&hours=900&name=`
    );
    if (request.kind !== "mint") throw new Error("expected a mint request");
    expect(request.ask).toEqual({ apps: ["acme/a", "acme/b"], hours: "900", name: "" });
  });

  it("tells a lifetime and a name oxyc didn't send from ones it sent empty", () => {
    const request = parse(`${PKCE}&kind=sandbox_agent`);
    if (request.kind !== "mint") throw new Error("expected a mint request");
    expect(request.ask).toEqual({ apps: [], hours: null, name: null });
  });

  it("never reads a kind it doesn't know as a login", () => {
    // Approving a login here would grant all access for a year to a client that asked for less.
    for (const kind of ["personal", "service_account", "SANDBOX_AGENT", ""]) {
      const request = parse(`${PKCE}&kind=${kind}&apps=acme/store-ops`);
      expect(request).toMatchObject({ kind: "invalid", title: "Token request failed" });
    }
  });

  it("never runs the token handoff for a mint that lost its code_challenge", () => {
    // With no `kind` this is the legacy flow, which hands over the session token with no click.
    expect(parse("port=53124&state=abc&kind=sandbox_agent&apps=acme/store-ops")).toMatchObject({
      kind: "invalid",
      title: "Token request failed"
    });
    expect(parse("port=53124&state=abc").kind).toBe("legacy");
  });

  it("refuses a mint whose PKCE params are broken, or whose values no oxyc would send", () => {
    for (const query of [
      `port=1&state=s&code_challenge=${CHALLENGE.slice(1)}&hostname=h&kind=sandbox_agent`,
      `port=1&state=s&code_challenge=${CHALLENGE}&kind=sandbox_agent&apps=acme/a`,
      `${PKCE}&kind=sandbox_agent&apps=acme/a&name=x%07y`,
      `${PKCE}&kind=sandbox_agent&apps=acme/a%0A&hours=8`
    ]) {
      expect(parse(query)).toMatchObject({ kind: "invalid", title: "Token request failed" });
    }
  });

  it("leaves a link with no kind exactly as it was: a login, with no mint title", () => {
    expect(parse(PKCE)).toEqual({
      kind: "pkce",
      port: "53124",
      state: "abc",
      codeChallenge: CHALLENGE,
      hostname: "luong-mbp"
    });
    // `apps`, `hours` and `name` alone ask for nothing: without `kind` they are ignored.
    expect(parse(`${PKCE}&apps=acme/store-ops&hours=8&name=x`).kind).toBe("pkce");
    const broken = parse(`port=1&state=s&code_challenge=${CHALLENGE}`);
    expect(broken).toEqual({
      kind: "invalid",
      reason: "This login link is incomplete. Run `oxyc login` again to get a new one."
    });
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

  it("carries a mint through login whole, so it never comes back as a login", () => {
    const pkce = `port=53124&state=a%20b&code_challenge=${CHALLENGE}&hostname=luong-mbp`;
    for (const mint of [
      "kind=sandbox_agent&apps=acme/store-ops,globex/pos&hours=8&name=refunds%20%26%20more",
      // Out of range and unresolvable as sent: what was wrong before login is wrong after it.
      "kind=sandbox_agent&apps=nope&hours=abc&name=",
      "kind=sandbox_agent"
    ]) {
      const request = parse(`${pkce}&${mint}`);
      if (request.kind !== "mint") throw new Error("expected a mint request");
      const back = parseCliAuthRequest(new URL(returnToUrl(origin, request)).searchParams);
      expect(back).toEqual(request);
    }
  });

  it("sends a mint's code to the loopback exactly as a login's", () => {
    const request = parse(
      `port=53124&state=abc&code_challenge=${CHALLENGE}&hostname=h&kind=sandbox_agent&apps=acme/a`
    );
    if (request.kind !== "mint") throw new Error("expected a mint request");
    expect(codeCallbackUrl(request, "one-time")).toBe(
      "http://127.0.0.1:53124/callback?code=one-time&state=abc"
    );
  });
});
