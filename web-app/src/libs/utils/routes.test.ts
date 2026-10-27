import { describe, expect, it } from "vitest";
import ROUTES from "./routes";

describe("IDE.EXPERIMENTS", () => {
  it("is nested under the workspace's ide in an org", () => {
    const routes = ROUTES.ORG("acme").WORKSPACE("ws1").IDE.EXPERIMENTS;
    expect(routes.ROOT).toBe("/acme/workspaces/ws1/ide/experiments");
    expect(routes.DETAIL("beverage_promo")).toBe(
      "/acme/workspaces/ws1/ide/experiments/beverage_promo"
    );
  });

  it("encodes the name and drops the prefix for an empty org slug", () => {
    const routes = ROUTES.ORG("").WORKSPACE("ws1").IDE.EXPERIMENTS;
    expect(routes.ROOT).toBe("/ide/experiments");
    expect(routes.DETAIL("a b")).toBe("/ide/experiments/a%20b");
  });
});
