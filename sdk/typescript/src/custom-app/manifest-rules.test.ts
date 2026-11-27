/**
 * The two server name rules and the screen-path rule, as `oxyc`, the Vite
 * plugin and the loader all apply them.
 */

import { describe, expect, it } from "vitest";
import {
  FUNCTION_NAME_PATTERN,
  isValidFunctionName,
  isValidScreenPath,
  isValidSlug,
  SCREEN_PATH_MAX_LEN,
  SCREEN_PATH_PATTERN,
  SLUG_MAX_LEN,
  SLUG_PATTERN
} from "./manifest-rules";

describe("isValidSlug", () => {
  it("accepts lower kebab up to 63 characters", () => {
    for (const ok of ["a", "store-pulse", "a1-b2", "oltp-bookings", "x".repeat(SLUG_MAX_LEN)]) {
      expect(isValidSlug(ok), ok).toBe(true);
    }
  });

  /** An underscore is the one people reach for; the slug becomes a schema name. */
  it("rejects every other shape", () => {
    for (const bad of ["", "-a", "a-", "a--b", "A", "a_b", "a b", "x".repeat(SLUG_MAX_LEN + 1)]) {
      expect(isValidSlug(bad), bad).toBe(false);
    }
  });

  it("is the pattern the schema carries, plus the length", () => {
    expect(SLUG_PATTERN.test("a--b")).toBe(false);
    expect(SLUG_PATTERN.test("x".repeat(100))).toBe(true);
  });
});

describe("isValidFunctionName", () => {
  it("follows the server's function-name grammar", () => {
    for (const ok of ["a", "top-stores", "a1", `a${"b".repeat(63)}`]) {
      expect(isValidFunctionName(ok), ok).toBe(true);
    }
    for (const bad of ["", "1a", "-a", "A", "a_b", "../x", `a${"b".repeat(64)}`]) {
      expect(isValidFunctionName(bad), bad).toBe(false);
    }
    expect(FUNCTION_NAME_PATTERN.source).toBe("^[a-z][a-z0-9-]{0,63}$");
  });
});

describe("isValidScreenPath", () => {
  it("accepts an app-relative path, the root included", () => {
    for (const ok of ["", "stores", "reorder/plans", "stores/", "a.b", "stores/12:30", "1:2"]) {
      expect(isValidScreenPath(ok), JSON.stringify(ok)).toBe(true);
    }
    expect(isValidScreenPath("x".repeat(SCREEN_PATH_MAX_LEN))).toBe(true);
  });

  it("accepts a query, for an app that picks its screen by one", () => {
    for (const ok of [
      "?module=waste",
      "stores?tab=open",
      "?module=waste&view=week",
      "stores?range=1..5",
      "?at=10:30",
      "?next=/stores"
    ]) {
      expect(isValidScreenPath(ok), JSON.stringify(ok)).toBe(true);
    }
    // The length is the whole entry's, query included.
    expect(isValidScreenPath(`?q=${"x".repeat(SCREEN_PATH_MAX_LEN - 3)}`)).toBe(true);
    expect(isValidScreenPath(`?q=${"x".repeat(SCREEN_PATH_MAX_LEN - 2)}`)).toBe(false);
  });

  it("rejects a leading slash, a hash, `..` in the path, a scheme and a query that is empty or spaced", () => {
    for (const bad of [
      "/",
      "/stores",
      "//example.test",
      "/?tab=1",
      "stores#top",
      "#/stores",
      "stores?tab=1#top",
      "?tab=1#top",
      "?",
      "stores?",
      "stores?a b",
      "?a=1\n",
      "..",
      "../x",
      "a/../b",
      "a..b",
      "../x?tab=1",
      "https://example.test/a",
      "https://example.test/a?b=1",
      "mailto:a@example.test",
      "stores:1"
    ]) {
      expect(isValidScreenPath(bad), JSON.stringify(bad)).toBe(false);
    }
  });

  it("is the pattern the schema carries, plus the length", () => {
    const long = "x".repeat(SCREEN_PATH_MAX_LEN + 1);
    expect(SCREEN_PATH_PATTERN.test(long)).toBe(true);
    expect(isValidScreenPath(long)).toBe(false);
  });
});
