import { describe, expect, it } from "vitest";
import ROUTES from "./routes";

describe("IDE.EXPERIMENTS", () => {
  it("is nested under the org's ide, naming no workspace", () => {
    const routes = ROUTES.ORG("acme").WORKSPACE().IDE.EXPERIMENTS;
    expect(routes.ROOT).toBe("/acme/ide/experiments");
    expect(routes.DETAIL("beverage_promo")).toBe("/acme/ide/experiments/beverage_promo");
  });

  it("encodes the name and drops the prefix for an empty org slug", () => {
    const routes = ROUTES.ORG("").WORKSPACE().IDE.EXPERIMENTS;
    expect(routes.ROOT).toBe("/ide/experiments");
    expect(routes.DETAIL("a b")).toBe("/ide/experiments/a%20b");
  });
});

describe("WORKSPACE", () => {
  it("addresses the org's pages by the org alone", () => {
    const ws = ROUTES.ORG("acme").WORKSPACE();
    expect(ws.THREAD("t-1")).toBe("/acme/threads/t-1");
    expect(ws.IDE.ROOT).toBe("/acme/ide");
  });

  // `/<org>` is the org dispatcher, so the workspace's landing is the home.
  it("lands on the org home, not the org root", () => {
    expect(ROUTES.ORG("acme").WORKSPACE().ROOT).toBe("/acme/home");
    expect(ROUTES.ORG("acme").WORKSPACE().ROOT).toBe(ROUTES.ORG("acme").HOME);
  });

  // `/<org>/onboarding` is the org's retired wizard, which redirects to the root.
  it("puts the setup wizard where the org's routes do not shadow it", () => {
    expect(ROUTES.ORG("acme").WORKSPACE().ONBOARDING).toBe("/acme/setup");
  });

  it("keeps local mode's flat addresses", () => {
    const ws = ROUTES.ORG("").WORKSPACE();
    expect(ws.ROOT).toBe("");
    expect(ws.THREADS).toBe("/threads");
  });
});

describe("ENTER_WORKSPACE", () => {
  it("names the workspace, for a link that opens that one", () => {
    const ws = ROUTES.ORG("acme").ENTER_WORKSPACE("ws1");
    expect(ws.ROOT).toBe("/acme/workspaces/ws1");
    expect(ws.IDE.ROOT).toBe("/acme/workspaces/ws1/ide");
    expect(ws.THREAD("t-1")).toBe("/acme/workspaces/ws1/threads/t-1");
  });

  it("is the plain address in local mode, which has one workspace", () => {
    expect(ROUTES.ORG("").ENTER_WORKSPACE("ws1").IDE.ROOT).toBe("/ide");
  });
});
