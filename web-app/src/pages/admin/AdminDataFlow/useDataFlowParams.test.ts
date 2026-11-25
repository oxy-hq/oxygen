import { describe, expect, it } from "vitest";
import { DEFAULT_DEPTH, patchSearch, readParams, toQuery } from "./useDataFlowParams";

const read = (query: string) => readParams(new URLSearchParams(query));

describe("readParams", () => {
  it("reads the console's deep link", () => {
    expect(read("org=o1&focus=app%3Aa1")).toEqual({
      org: "o1",
      workspace: null,
      focus: "app:a1",
      depth: DEFAULT_DEPTH,
      level: "schema"
    });
  });

  it("falls back to the default for a value it cannot use", () => {
    expect(read("org=o&depth=9").depth).toBe(DEFAULT_DEPTH);
    expect(read("org=o&depth=abc").depth).toBe(DEFAULT_DEPTH);
    expect(read("org=o&level=column").level).toBe("schema");
    expect(read("org=o&depth=4&level=table")).toMatchObject({ depth: 4, level: "table" });
  });
});

describe("toQuery", () => {
  it("asks nothing until an org is chosen", () => {
    expect(toQuery(read(""))).toBeNull();
  });

  it("leaves defaults out, and depth out unless focused", () => {
    expect(toQuery(read("org=o&depth=4"))).toEqual({ org_id: "o" });
    expect(toQuery(read("org=o&focus=app%3Aa&depth=4&workspace=w&level=table"))).toEqual({
      org_id: "o",
      workspace_id: "w",
      focus: "app:a",
      depth: 4,
      level: "table"
    });
  });
});

describe("patchSearch", () => {
  const patched = (query: string, patch: Parameters<typeof patchSearch>[1]) =>
    patchSearch(new URLSearchParams(query), patch).toString();

  it("sets what the patch names and keeps the rest", () => {
    expect(patched("org=o&level=table", { focus: "app:a" })).toBe(
      "org=o&level=table&focus=app%3Aa"
    );
  });

  it("drops a null and a value equal to its default", () => {
    expect(
      patched("org=o&focus=f&depth=4&level=table", { focus: null, depth: 2, level: "schema" })
    ).toBe("org=o");
  });

  it("a new org clears the workspace and the focus with it", () => {
    expect(patched("org=o&workspace=w&focus=f", { org: "p", workspace: null, focus: null })).toBe(
      "org=p"
    );
  });
});
