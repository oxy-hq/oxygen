import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import {
  approveByDeviceCode,
  ciName,
  exportedApproverApplies,
  headlessReason
} from "./headless.js";

const loginByDeviceCode = (
  browser: boolean | undefined,
  noWait: boolean,
  env: Record<string, string | undefined>,
  platform: NodeJS.Platform
) => approveByDeviceCode(browser, { noWait, what: "the login" }, env, platform);

const DESKTOP_LINUX = { DISPLAY: ":0" };

describe("headlessReason", () => {
  it("finds a browser on a Mac, on Windows, and on a Linux desktop", () => {
    expect(headlessReason({}, "darwin")).toBeUndefined();
    expect(headlessReason({}, "win32")).toBeUndefined();
    expect(headlessReason(DESKTOP_LINUX, "linux")).toBeUndefined();
    expect(headlessReason({ WAYLAND_DISPLAY: "wayland-0" }, "linux")).toBeUndefined();
  });

  it("finds none in a Claude Code remote session, even with an opener configured", () => {
    expect(headlessReason({ CLAUDE_CODE_REMOTE: "true", BROWSER: "x" }, "linux")).toMatch(
      /Claude Code remote session/
    );
    expect(headlessReason({ CLAUDE_CODE_REMOTE: "false", ...DESKTOP_LINUX }, "linux")).toBe(
      undefined
    );
  });

  it("finds none in a Codespace, over SSH, or on a Linux box with no display", () => {
    expect(headlessReason({ CODESPACES: "true" }, "linux")).toMatch(/Codespace/);
    expect(headlessReason({ SSH_CONNECTION: "1.2.3.4 5 6.7.8.9 22" }, "darwin")).toMatch(/SSH/);
    expect(headlessReason({ SSH_TTY: "/dev/pts/0" }, "linux")).toMatch(/SSH/);
    expect(headlessReason({}, "linux")).toMatch(/no display/);
  });

  it("trusts an explicit opener over SSH (VS Code Remote-SSH forwards the port)", () => {
    expect(headlessReason({ SSH_CONNECTION: "x", BROWSER: "/vscode/helper" }, "linux")).toBe(
      undefined
    );
  });

  it("finds a browser under WSL, which opens the Windows one", () => {
    expect(headlessReason({ WSL_DISTRO_NAME: "Ubuntu" }, "linux")).toBeUndefined();
  });

  it("obeys OXYC_HEADLESS over every signal", () => {
    expect(headlessReason({ OXYC_HEADLESS: "1" }, "darwin")).toMatch(/OXYC_HEADLESS/);
    expect(headlessReason({ OXYC_HEADLESS: "0", CLAUDE_CODE_REMOTE: "1" }, "linux")).toBe(
      undefined
    );
    // Empty is unset, not "off".
    expect(headlessReason({ OXYC_HEADLESS: "", SSH_TTY: "x" }, "linux")).toMatch(/SSH/);
  });
});

describe("loginByDeviceCode", () => {
  let said: string;
  beforeEach(() => {
    said = "";
    vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
      said += String(chunk);
      return true;
    });
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("does what an explicit flag says, whatever the machine", () => {
    expect(loginByDeviceCode(true, false, { CLAUDE_CODE_REMOTE: "1" }, "linux")).toBe(false);
    expect(loginByDeviceCode(false, false, {}, "darwin")).toBe(true);
    expect(said).toBe("");
  });

  it("is always the device code for --no-wait", () => {
    expect(loginByDeviceCode(undefined, true, {}, "darwin")).toBe(true);
  });

  it("switches on a headless machine, and says why and how to force the browser", () => {
    expect(loginByDeviceCode(undefined, false, { CLAUDE_CODE_REMOTE: "true" }, "linux")).toBe(true);
    expect(said).toMatch(/Claude Code remote session/);
    expect(said).toContain("--browser");
  });

  it("keeps the browser where one can reach the loopback", () => {
    expect(loginByDeviceCode(undefined, false, {}, "darwin")).toBe(false);
    expect(said).toBe("");
  });
});

describe("exportedApproverApplies", () => {
  it("applies where the device code was chosen: --no-browser, --no-wait, a cloud session", () => {
    expect(exportedApproverApplies(false, false, {})).toBe(true);
    expect(exportedApproverApplies(undefined, true, {})).toBe(true);
    expect(exportedApproverApplies(undefined, false, { CLAUDE_CODE_REMOTE: "true" })).toBe(true);
  });

  it("does not where it was only detected, or the browser was chosen", () => {
    expect(exportedApproverApplies(undefined, false, { SSH_TTY: "/dev/pts/0" })).toBe(false);
    expect(exportedApproverApplies(undefined, false, { CODESPACES: "true" })).toBe(false);
    expect(exportedApproverApplies(true, false, { CLAUDE_CODE_REMOTE: "true" })).toBe(false);
  });
});

describe("approveByDeviceCode on a CI runner", () => {
  const LOGIN = { what: "the login" };
  const refusal = (fn: () => unknown): CliError => {
    try {
      fn();
    } catch (cause) {
      expect(cause).toBeInstanceOf(CliError);
      return cause as CliError;
    }
    throw new Error("expected a refusal");
  };

  it("names the CI system", () => {
    expect(ciName({ GITHUB_ACTIONS: "true", CI: "true" })).toBe("GitHub Actions");
    expect(ciName({ GITLAB_CI: "true" })).toBe("GitLab CI");
    expect(ciName({ CI: "1" })).toBe("CI");
    expect(ciName({ CI: "false" })).toBeUndefined();
    expect(ciName({})).toBeUndefined();
  });

  it("refuses at once, as AUTH, naming the credentials that need no person", () => {
    // Even on a Mac runner, where nothing else says headless.
    for (const platform of ["linux", "darwin"] as const) {
      const cause = refusal(() =>
        approveByDeviceCode(undefined, LOGIN, { CI: "true", GITHUB_ACTIONS: "true" }, platform)
      );
      expect(cause.code).toBe(ExitCode.AUTH);
      expect(cause.message).toContain("GitHub Actions");
      expect(cause.hint).toContain("OXY_TOKEN");
      expect(cause.hint).toContain("--service-account");
      expect(cause.hint).toContain("--approver");
    }
  });

  it("goes by device code when an approver is named, or a flag or OXYC_HEADLESS chose", () => {
    const ci = { CI: "true" };
    expect(approveByDeviceCode(undefined, { ...LOGIN, approverNamed: true }, ci, "linux")).toBe(
      true
    );
    expect(approveByDeviceCode(false, LOGIN, ci, "linux")).toBe(true);
    expect(approveByDeviceCode(true, LOGIN, ci, "linux")).toBe(false);
    expect(approveByDeviceCode(undefined, { ...LOGIN, noWait: true }, ci, "linux")).toBe(true);
    expect(approveByDeviceCode(undefined, LOGIN, { ...ci, OXYC_HEADLESS: "1" }, "linux")).toBe(
      true
    );
    expect(approveByDeviceCode(undefined, LOGIN, { ...ci, OXYC_HEADLESS: "0" }, "linux")).toBe(
      false
    );
  });

  it("treats a Claude Code remote session as a person at the other end, not CI", () => {
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    expect(
      approveByDeviceCode(undefined, LOGIN, { CI: "true", CLAUDE_CODE_REMOTE: "true" }, "linux")
    ).toBe(true);
    vi.restoreAllMocks();
  });
});
