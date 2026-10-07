// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { isStoredTokenSession, storedUserLabel } from "./authStorage";

const segment = (value: unknown) =>
  btoa(JSON.stringify(value)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** A JWT as the server signs one: the header is where a token session's `kid` lives. */
const jwt = (header: unknown, claims: unknown = { sub: "user-1" }) =>
  `${segment(header)}.${segment(claims)}.signature`;

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

// `/token-login` asks before replacing a person's session and never asks an
// agent, and this is the whole of how it tells them apart: a session opened
// with an API token has a header `kid` of `tok:<id>`, any other has none.
describe("isStoredTokenSession", () => {
  it("is true for a session whose header kid names an API token", () => {
    localStorage.setItem(
      "auth_token",
      jwt({ alg: "HS256", typ: "JWT", kid: "tok:6f1c2a4e-9b1d-4c53-8a0e-2f6d1b7c9e10" })
    );
    expect(isStoredTokenSession()).toBe(true);
  });

  it("is false for an ordinary session, which has no kid", () => {
    localStorage.setItem("auth_token", jwt({ alg: "HS256", typ: "JWT" }));
    expect(isStoredTokenSession()).toBe(false);
  });

  it("is false for a kid of another kind", () => {
    localStorage.setItem("auth_token", jwt({ alg: "HS256", kid: "2026-10" }));
    expect(isStoredTokenSession()).toBe(false);
    // Only a string can be a token kid; `startsWith` on anything else would throw.
    localStorage.setItem("auth_token", jwt({ alg: "HS256", kid: 7 }));
    expect(isStoredTokenSession()).toBe(false);
  });

  it("reads the header, not the claims", () => {
    // A `kid` claim proves nothing: the server writes the marker in the header.
    localStorage.setItem("auth_token", jwt({ alg: "HS256" }, { sub: "user-1", kid: "tok:abc" }));
    expect(isStoredTokenSession()).toBe(false);
  });

  it("is false when nothing is stored", () => {
    expect(isStoredTokenSession()).toBe(false);
  });

  it("never throws on a token it cannot parse", () => {
    for (const garbage of ["", "not-a-jwt", "a.b.c", "%%%.%%%.%%%", `${segment("tok:abc")}.x.y`]) {
      localStorage.setItem("auth_token", garbage);
      expect(isStoredTokenSession()).toBe(false);
    }
    localStorage.setItem("auth_token", `${segment(null)}.x.y`);
    expect(isStoredTokenSession()).toBe(false);
  });

  it("never throws when storage itself is unavailable", () => {
    // The prototype, not the instance: assigning a property on a `Storage`
    // stores an item of that name instead of replacing the method.
    vi.spyOn(Object.getPrototypeOf(localStorage), "getItem").mockImplementation(() => {
      throw new DOMException("denied", "SecurityError");
    });
    expect(isStoredTokenSession()).toBe(false);
  });
});

describe("storedUserLabel", () => {
  it("names the stored user by email", () => {
    localStorage.setItem("user", JSON.stringify({ email: "maya@acme.test", name: "Maya" }));
    expect(storedUserLabel()).toBe("maya@acme.test");
  });

  it("falls back to the name when there is no email", () => {
    localStorage.setItem("user", JSON.stringify({ email: "", name: "Maya" }));
    expect(storedUserLabel()).toBe("Maya");
  });

  it("is null when there is no stored user, or it cannot be read", () => {
    expect(storedUserLabel()).toBeNull();
    for (const stored of ["{not json", "null", '"maya"', "{}", '{"email":7}']) {
      localStorage.setItem("user", stored);
      expect(storedUserLabel()).toBeNull();
    }
  });
});
