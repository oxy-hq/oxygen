import { describe, expect, it } from "vitest";
import ROUTES from "./routes";
import { entryRedirectPath } from "./workspaceEntry";

const WS = "b92f6ff7-a605-4566-a4f0-5448f1127d09";

describe("entryRedirectPath", () => {
  it("drops the workspace from a page's address", () => {
    expect(entryRedirectPath(`/oxygen/workspaces/${WS}/threads/t-1`)).toBe("/oxygen/threads/t-1");
    expect(entryRedirectPath(`/oxygen/workspaces/${WS}/ide/coordinator/runs/r-9`)).toBe(
      "/oxygen/ide/coordinator/runs/r-9"
    );
  });

  it("sends the workspace's root, with or without a slash, to the org home", () => {
    expect(entryRedirectPath(`/oxygen/workspaces/${WS}`)).toBe("/oxygen/home");
    expect(entryRedirectPath(`/oxygen/workspaces/${WS}/`)).toBe("/oxygen/home");
  });

  it("keeps an encoded segment as it was sent", () => {
    expect(entryRedirectPath(`/acme/workspaces/${WS}/ide/experiments/a%20b`)).toBe(
      "/acme/ide/experiments/a%20b"
    );
  });

  it("moves the setup page off `onboarding`, the org's retired wizard", () => {
    expect(entryRedirectPath(`/acme/workspaces/${WS}/onboarding`)).toBe("/acme/setup");
  });

  it("leaves anything else alone", () => {
    expect(entryRedirectPath("/acme/threads")).toBeNull();
    expect(entryRedirectPath("/acme/workspaces")).toBeNull();
  });

  // Every address an entry link can be built with lands on the same page the
  // shell's own links point at.
  it("lands every entry link on the org address of the same page", () => {
    const entry = ROUTES.ORG("acme").ENTER_WORKSPACE(WS);
    const org = ROUTES.ORG("acme").WORKSPACE();
    const pairs: [string, string][] = [
      [entry.ROOT, org.ROOT],
      [entry.HOME, org.HOME],
      [entry.THREAD("t-1"), org.THREAD("t-1")],
      [entry.AUTOMATION("YQ").ROOT, org.AUTOMATION("YQ").ROOT],
      [entry.IDE.SEMANTIC.ROOT, org.IDE.SEMANTIC.ROOT],
      [entry.IDE.EXPERIMENTS.DETAIL("a b"), org.IDE.EXPERIMENTS.DETAIL("a b")],
      [entry.ONBOARDING, org.ONBOARDING],
      [entry.WORLD_MODEL, org.WORLD_MODEL]
    ];
    for (const [from, to] of pairs) expect(entryRedirectPath(from)).toBe(to);
  });
});
