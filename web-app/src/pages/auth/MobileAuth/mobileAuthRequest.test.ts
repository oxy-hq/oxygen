import { describe, expect, it } from "vitest";
import {
  cleanRedirectUri,
  codeRedirectUrl,
  declinedRedirectUrl,
  loginUrl,
  parseMobileAuthRequest,
  returnToUrl
} from "./mobileAuthRequest";

const CHALLENGE = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

const parse = (query: string) => parseMobileAuthRequest(new URLSearchParams(query));
const ok = (query: string) => {
  const request = parse(query);
  if (request.kind !== "ok") throw new Error(`expected a request, got: ${request.reason}`);
  return request;
};

describe("cleanRedirectUri", () => {
  it.each([
    "oxygen://auth",
    "oxygen://auth-dev",
    " oxygen://auth ",
    "oxygen://auth/callback",
    "OXYGEN://auth"
  ])("accepts %s", (uri) => {
    expect(cleanRedirectUri(uri)).toBe(uri.trim());
  });

  it.each([
    null,
    "",
    "exp://192.168.1.4:8081/--/auth",
    "exp+oxygen://auth",
    "https://evil.example.com/cb",
    "javascript:alert(1)",
    "oxygen:auth",
    "oxygenx://auth",
    "oxygen://auth?next=x",
    "oxygen://auth#x",
    "oxygen://user@auth",
    "oxygen://a b",
    `oxygen://${"a".repeat(512)}`
  ])("refuses %s", (uri) => {
    expect(cleanRedirectUri(uri)).toBeNull();
  });
});

describe("parseMobileAuthRequest", () => {
  it("reads the challenge, redirect and state", () => {
    expect(ok(`code_challenge=${CHALLENGE}&redirect_uri=oxygen://auth&state=s1`)).toEqual({
      kind: "ok",
      codeChallenge: CHALLENGE,
      redirectUri: "oxygen://auth",
      state: "s1",
      provider: null
    });
  });

  it("reads the provider the person picked, and nothing it doesn't know", () => {
    const base = `code_challenge=${CHALLENGE}&redirect_uri=oxygen://auth`;
    expect(ok(`${base}&provider=google`).provider).toBe("google");
    expect(ok(`${base}&provider=GitHub`).provider).toBe("github");
    expect(ok(`${base}&provider=okta`).provider).toBe("okta");
    expect(ok(`${base}&provider=email`).provider).toBe("email");
    expect(ok(`${base}&provider=myspace`).provider).toBeNull();
    expect(ok(base).provider).toBeNull();
  });

  it("drops a challenge's padding and keeps a missing state missing", () => {
    const request = ok(`code_challenge=${CHALLENGE}%3D&redirect_uri=oxygen%3A%2F%2Fauth`);
    expect(request.codeChallenge).toBe(CHALLENGE);
    expect(request.state).toBeNull();
  });

  it("refuses a link with a foreign redirect or no usable challenge", () => {
    expect(parse(`code_challenge=${CHALLENGE}&redirect_uri=https://evil.example.com`).kind).toBe(
      "invalid"
    );
    expect(parse(`code_challenge=${CHALLENGE}`).kind).toBe("invalid");
    expect(parse("code_challenge=short&redirect_uri=oxygen://auth").kind).toBe("invalid");
    expect(parse("redirect_uri=oxygen://auth").kind).toBe("invalid");
  });
});

describe("the URLs the page navigates to", () => {
  const request = ok(`code_challenge=${CHALLENGE}&redirect_uri=oxygen://auth&state=a%26b`);

  it("returns the code and the state, encoded", () => {
    expect(codeRedirectUrl(request, "c/d+e")).toBe("oxygen://auth?code=c%2Fd%2Be&state=a%26b");
    expect(declinedRedirectUrl(request)).toBe("oxygen://auth?error=access_denied&state=a%26b");
  });

  it("sends no state when the app sent none", () => {
    const stateless = ok(`code_challenge=${CHALLENGE}&redirect_uri=oxygen://auth`);
    expect(codeRedirectUrl(stateless, "c")).toBe("oxygen://auth?code=c");
  });

  it("comes back from login to this exact request", () => {
    const returnTo = returnToUrl("https://oxygen-hq.com", request);
    const back = new URL(returnTo);
    expect(back.pathname).toBe("/mobile-auth");
    expect(parseMobileAuthRequest(back.searchParams)).toEqual(request);
    expect(loginUrl(returnTo)).toBe(`/login?return_to=${encodeURIComponent(returnTo)}`);
    expect(loginUrl(returnTo, "email")).toBe(
      `/login?return_to=${encodeURIComponent(returnTo)}&focus=email`
    );
  });

  it("comes back without the provider, so a failed sign-in stops at the login page", () => {
    const picked = ok(`code_challenge=${CHALLENGE}&redirect_uri=oxygen://auth&provider=google`);
    const back = new URL(returnToUrl("https://oxygen-hq.com", picked));
    expect(back.searchParams.has("provider")).toBe(false);
    expect(parseMobileAuthRequest(back.searchParams)).toEqual({ ...picked, provider: null });
  });
});
