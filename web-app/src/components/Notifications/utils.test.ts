import { describe, expect, it } from "vitest";
import type { InboxItem } from "@/types/notifications";
import { isAppHref, sourceLine } from "./utils";

describe("isAppHref", () => {
  it("knows a link into a custom app", () => {
    expect(isAppHref("/customer-apps/acme/store-ops/lists/42")).toBe(true);
    expect(isAppHref("/customer-apps/acme/store-ops/")).toBe(true);
    expect(isAppHref("/customer-apps/acme/store-ops")).toBe(true);
    expect(isAppHref("/customer-apps/acme/store-ops?tab=overdue")).toBe(true);
  });

  it("leaves a path inside the product to the router", () => {
    expect(isAppHref("/acme/workspaces/ws-1/threads")).toBe(false);
    expect(isAppHref("/work/1")).toBe(false);
    // Not an app: no app segment.
    expect(isAppHref("/customer-apps/acme")).toBe(false);
  });
});

describe("sourceLine", () => {
  const item = (over: Partial<InboxItem>): InboxItem => ({
    id: "n",
    org_id: "o",
    kind: "temp_alarm",
    created_at: "2026-10-14T11:55:00Z",
    read_at: null,
    title: "t",
    body: null,
    href: null,
    link: null,
    app: { id: "a", slug: "store-ops", name: "Store Ops" },
    ...over
  });

  it("names the source and the category's own label", () => {
    expect(
      sourceLine(
        item({
          category: {
            path: "operations/food-safety",
            labels: ["Store operations", "Food safety"]
          }
        })
      )
    ).toBe("Store Ops · Food safety");
  });

  it("is Oxygen for a platform notice", () => {
    expect(
      sourceLine(item({ app: null, category: { path: "billing", labels: ["Billing"] } }))
    ).toBe("Oxygen · Billing");
  });

  it("leaves out the implicit General category", () => {
    expect(sourceLine(item({ category: { path: "general", labels: ["General"] } }))).toBe(
      "Store Ops"
    );
  });

  it("is just the source from a server that sends no category", () => {
    expect(sourceLine(item({}))).toBe("Store Ops");
    expect(sourceLine(item({ app: null }))).toBe("Oxygen");
  });
});
