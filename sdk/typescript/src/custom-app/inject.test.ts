import { afterEach, describe, expect, it, vi } from "vitest";

import { readAppEnvironment, readInjectedAppConfig } from "./inject";

const identity = {
  appId: "a1",
  slug: "store",
  orgId: "o1",
  orgSlug: "acme",
  projectId: "p1",
  branch: "main",
  apiBaseUrl: ""
};

describe("readAppEnvironment", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads the environment the server injected", () => {
    vi.stubGlobal("window", { __OXY_APP__: { ...identity, environment: "staging" } });
    expect(readAppEnvironment()).toBe("staging");
    expect(readInjectedAppConfig()?.environment).toBe("staging");
  });

  it("is undefined, not production, when an older server injected none", () => {
    vi.stubGlobal("window", { __OXY_APP__: identity });
    expect(readAppEnvironment()).toBeUndefined();
  });

  it("is undefined when oxy did not serve the page", () => {
    vi.stubGlobal("window", {});
    expect(readAppEnvironment()).toBeUndefined();
  });
});
