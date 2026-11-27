/**
 * Who a claim says is running the checks: a GitHub Actions run and its link,
 * or the command run by hand.
 */

import { describe, expect, it } from "vitest";
import { runnerIdentity } from "./runner-identity.js";

const GITHUB = {
  GITHUB_ACTIONS: "true",
  GITHUB_RUN_NUMBER: "1482",
  GITHUB_RUN_ID: "123",
  GITHUB_SERVER_URL: "https://github.com",
  GITHUB_REPOSITORY: "oxy-hq/oxygen-internal"
};

describe("runnerIdentity", () => {
  it("names the GitHub Actions run and links to it", () => {
    expect(runnerIdentity(GITHUB)).toEqual({
      label: "GitHub Actions run #1482",
      url: "https://github.com/oxy-hq/oxygen-internal/actions/runs/123"
    });
  });

  it("is a run by hand, with no link, outside GitHub Actions", () => {
    expect(runnerIdentity({})).toEqual({ label: "oxyc, run by hand", url: null });
    // The run variables alone are not GitHub Actions.
    expect(runnerIdentity({ ...GITHUB, GITHUB_ACTIONS: undefined })).toEqual({
      label: "oxyc, run by hand",
      url: null
    });
  });

  it("gives no link rather than a wrong one when a variable is not what GitHub writes", () => {
    for (const broken of [
      { GITHUB_RUN_ID: "123/../../evil" },
      { GITHUB_RUN_ID: undefined },
      { GITHUB_REPOSITORY: "oxy-hq/oxygen-internal/pulls?x=" },
      { GITHUB_SERVER_URL: "javascript:alert(1)" },
      { GITHUB_SERVER_URL: "http://github.com" },
      { GITHUB_SERVER_URL: "not a url" }
    ]) {
      expect(runnerIdentity({ ...GITHUB, ...broken }), JSON.stringify(broken)).toEqual({
        label: "GitHub Actions run #1482",
        url: null
      });
    }
    expect(runnerIdentity({ ...GITHUB, GITHUB_RUN_NUMBER: "12 <b>" }).label).toBe("GitHub Actions");
  });
});
