import { describe, expect, it } from "vitest";
import { mergeLocation } from "./AdminRedirect";

describe("mergeLocation", () => {
  it("keeps the hash the old link carried", () => {
    expect(mergeLocation("/admin/observability/weekly", "", "#org-acme")).toBe(
      "/admin/observability/weekly#org-acme"
    );
  });

  it("adds the old query to the target's own", () => {
    expect(mergeLocation("/admin/tokens?kind=publish", "?q=ci", "")).toBe(
      "/admin/tokens?kind=publish&q=ci"
    );
  });

  it("lets the target's own query win a clash", () => {
    expect(mergeLocation("/admin/tenants?type=users", "?type=orgs", "")).toBe(
      "/admin/tenants?type=users"
    );
  });

  it("leaves a bare target bare", () => {
    expect(mergeLocation("/admin", "", "")).toBe("/admin");
  });
});
