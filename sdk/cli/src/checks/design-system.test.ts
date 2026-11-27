/**
 * The `design_system` verdict: the SDK floor, and what a theme finding does to
 * it. The guard itself is the SDK's and is stood in for here; that the real
 * one is the one bundled, and is read in built-CSS mode, is
 * `design-system-guard.test.ts`.
 */

import { checkThemeCss, type ThemeFinding } from "@oxy-hq/sdk/theme-guard";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { checkDesignSystem, judgeDesignSystem, meetsFloor, parseVersion } from "./design-system.js";

vi.mock("@oxy-hq/sdk/theme-guard", () => ({ checkThemeCss: vi.fn(() => []) }));

const guard = vi.mocked(checkThemeCss);
const FLOOR = "2.30.0";
const SOURCES = [
  { file: "index.html", css: "<style>a{}</style>" },
  { file: "assets/index-abc.css", css: ".app{color:var(--card)}" }
];

function restated(line: number, token: string): ThemeFinding {
  return {
    file: "assets/index-abc.css",
    line,
    at: `assets/index-abc.css:${line}`,
    rule: "restates-sdk-token",
    subject: token,
    message: `restates ${token}, which theme.css defines; remove it`
  };
}

beforeEach(() => {
  guard.mockReset();
  guard.mockReturnValue([]);
});

describe("checkDesignSystem", () => {
  it("hands the build's sources to the guard in built-CSS mode", () => {
    checkDesignSystem(SOURCES, "2.31.0", FLOOR);
    expect(guard).toHaveBeenCalledExactlyOnceWith(SOURCES, { built: true });
  });

  it("passes a clean bundle on a current SDK", () => {
    const out = checkDesignSystem(SOURCES, "2.30.1", FLOOR);
    expect(out).toEqual({
      status: "passed",
      summary: "SDK 2.30.1 · guard clean",
      detail: {
        findings: [],
        sdk_version: "2.30.1",
        sdk_floor: "2.30.0",
        files: ["index.html", "assets/index-abc.css"]
      }
    });
  });

  it("skips a build with no recorded SDK version when the guard is clean", () => {
    const out = checkDesignSystem(SOURCES, null, FLOOR);
    expect(out.status).toBe("skipped");
    expect(out.summary).toBe(
      "SDK version unknown: the build does not say which @oxy-hq/sdk it was built with (an oxyc that sends it records it; the floor is 2.30.0)"
    );
    // Unknown is no verdict on the SDK: the finding informs.
    expect(out.detail.findings).toEqual([
      { severity: "info", message: out.summary, at: null, rule: "sdk-floor" }
    ]);
    expect(out.detail.sdk_version).toBeNull();
  });

  it("fails a build whose SDK is below the floor", () => {
    const out = checkDesignSystem(SOURCES, "2.29.4", FLOOR);
    expect(out.status).toBe("failed");
    expect(out.summary).toBe("@oxy-hq/sdk 2.29.4 is below the floor 2.30.0");
    expect(out.detail.findings).toEqual([
      { severity: "error", message: out.summary, at: null, rule: "sdk-floor" }
    ]);
  });

  it("fails on a guard finding, and names its line", () => {
    guard.mockReturnValue([restated(2, "--card")]);
    const out = checkDesignSystem(SOURCES, "2.30.0", FLOOR);
    expect(out.status).toBe("failed");
    expect(out.summary).toBe("assets/index-abc.css:2 restates --card");
    expect(out.detail.findings).toEqual([
      {
        severity: "error",
        message: "restates --card, which theme.css defines; remove it",
        at: "assets/index-abc.css:2",
        rule: "restates-sdk-token"
      }
    ]);
  });

  it("fails on a guard finding whatever the SDK version, and counts the rest", () => {
    guard.mockReturnValue([
      restated(2, "--card"),
      { ...restated(9, ".dark .x"), rule: "bare-mode-class" }
    ]);
    // No recorded version would be `skipped`; a finding is a verdict regardless.
    const unknown = checkDesignSystem(SOURCES, null, FLOOR);
    expect(unknown.status).toBe("failed");
    expect(unknown.summary).toBe("assets/index-abc.css:2 restates --card (+1 more)");
    expect(unknown.detail.findings.map((f) => [f.severity, f.rule])).toEqual([
      ["error", "restates-sdk-token"],
      ["error", "bare-mode-class"],
      ["info", "sdk-floor"]
    ]);
    // Below the floor as well: the guard's finding is still the one named.
    const below = checkDesignSystem(SOURCES, "2.0.0", FLOOR);
    expect(below.status).toBe("failed");
    expect(below.summary).toBe("assets/index-abc.css:2 restates --card (+1 more)");
    expect(below.detail.findings.at(-1)).toMatchObject({ severity: "error", rule: "sdk-floor" });
  });

  it("names a rule other than a restated token by its id", () => {
    const found = { ...restated(4, ".dark .card"), rule: "bare-mode-class" };
    expect(judgeDesignSystem("2.31.0", FLOOR, [found], []).summary).toBe(
      "assets/index-abc.css:4 bare-mode-class: .dark .card"
    );
  });

  it("cannot judge a recorded version, or a floor, that is no version", () => {
    const sdk = checkDesignSystem(SOURCES, "latest", FLOOR);
    expect(sdk.status).toBe("error");
    expect(sdk.summary).toBe('@oxy-hq/sdk "latest" is not a version');
    const floor = checkDesignSystem(SOURCES, "2.31.0", "soon");
    expect(floor.status).toBe("error");
    expect(floor.summary).toBe('the platform\'s SDK floor "soon" is not a version');
  });
});

describe("the SDK floor", () => {
  it("compares the first major.minor.patch in a version", () => {
    const floor = parseVersion(FLOOR);
    expect(floor).toEqual([2, 30, 0]);
    if (!floor) throw new Error("the floor parses");
    for (const at of ["2.30.0", "2.31.4", "3.0.0", "^2.30.0", "2.30.0-rc.1", "v2.100.0"]) {
      expect(meetsFloor(at, floor), at).toBe(true);
    }
    for (const below of ["2.29.9", "1.99.99", "~2.9.30"]) {
      expect(meetsFloor(below, floor), below).toBe(false);
    }
    for (const none of ["latest", "2.30", ""])
      expect(meetsFloor(none, floor), none).toBeUndefined();
  });
});
