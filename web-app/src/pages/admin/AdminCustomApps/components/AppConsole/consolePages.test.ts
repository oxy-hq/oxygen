import { describe, expect, it } from "vitest";
import { CONSOLE_PAGES, consolePageFrom, consolePagePath } from "./consolePages";

const app = { org_slug: "acme", slug: "store-ops" };

describe("consolePageFrom", () => {
  it("reads the bare app route as the overview", () => {
    expect(consolePageFrom(undefined)).toBe("overview");
    expect(consolePageFrom("")).toBe("overview");
  });

  it("reads each page's own segment as that page", () => {
    for (const page of CONSOLE_PAGES) {
      expect(consolePageFrom(page.segment || undefined)).toBe(page.id);
    }
  });

  it("names no page for a segment none has, rather than falling back to the overview", () => {
    expect(consolePageFrom("logs")).toBeNull();
    expect(consolePageFrom("overview")).toBeNull();
  });
});

describe("consolePagePath", () => {
  it("puts the overview on the bare app route", () => {
    expect(consolePagePath(app, "overview")).toBe("/admin/apps/acme/store-ops");
  });

  it("puts every other page one segment under the app", () => {
    expect(consolePagePath(app, "events")).toBe("/admin/apps/acme/store-ops/events");
  });

  it("keeps the environment switch across pages", () => {
    expect(consolePagePath(app, "functions", "?env=staging")).toBe(
      "/admin/apps/acme/store-ops/functions?env=staging"
    );
  });

  it("drops one page's selection when moving to another", () => {
    expect(consolePagePath(app, "events", "?env=staging&fn=checkout&section=x")).toBe(
      "/admin/apps/acme/store-ops/events?env=staging"
    );
  });

  it("adds a selection the link opens the page on", () => {
    expect(consolePagePath(app, "functions", "?env=dev-ana", { fn: "checkout" })).toBe(
      "/admin/apps/acme/store-ops/functions?env=dev-ana&fn=checkout"
    );
  });

  it("encodes the slugs it builds the path from", () => {
    expect(consolePagePath({ org_slug: "a b", slug: "c/d" }, "data")).toBe(
      "/admin/apps/a%20b/c%2Fd/data"
    );
  });

  it("gives every page a distinct address", () => {
    const paths = CONSOLE_PAGES.map((p) => consolePagePath(app, p.id));
    expect(new Set(paths).size).toBe(CONSOLE_PAGES.length);
  });
});
