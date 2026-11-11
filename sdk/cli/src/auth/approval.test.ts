import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ExitCode } from "../util/errors.js";
import { resolveApproval } from "./approval.js";

const LEAD = { approver: "lead@example.com" };
const LOGIN = { what: "the login" };

describe("resolveApproval", () => {
  beforeEach(() => {
    vi.spyOn(process.stderr, "write").mockImplementation(() => true);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends an exported approver with --no-wait, which chooses the device code", () => {
    expect(resolveApproval(LEAD, "env", { ...LOGIN, noWait: true }, {}, "darwin")).toEqual({
      noBrowser: true,
      approver: "lead@example.com"
    });
  });

  it("sends an exported approver in a Claude Code cloud session, configured on purpose", () => {
    const env = { CLAUDE_CODE_REMOTE: "true" };
    expect(resolveApproval(LEAD, "env", LOGIN, env, "linux")).toEqual({
      noBrowser: true,
      approver: "lead@example.com"
    });
    expect(resolveApproval(LEAD, "env", { what: "the token" }, env, "linux").approver).toBe(
      "lead@example.com"
    );
  });

  it("sends an exported approver with an explicit --no-browser", () => {
    expect(resolveApproval({ ...LEAD, browser: false }, "env", LOGIN, {}, "darwin")).toEqual({
      noBrowser: true,
      approver: "lead@example.com"
    });
  });

  it("drops an exported approver where the device code was only detected (SSH)", () => {
    expect(resolveApproval(LEAD, "env", LOGIN, { SSH_TTY: "/dev/pts/0" }, "linux")).toEqual({
      noBrowser: true,
      approver: undefined
    });
  });

  it("keeps the browser, and ignores an exported approver, on a desktop", () => {
    expect(resolveApproval(LEAD, "env", LOGIN, {}, "darwin")).toEqual({
      noBrowser: false,
      approver: undefined
    });
  });

  it("always honours the flag on the device code, detected or not", () => {
    expect(resolveApproval(LEAD, "cli", LOGIN, { SSH_TTY: "x" }, "linux").approver).toBe(
      "lead@example.com"
    );
  });

  it("refuses on a CI runner with only an exported approver, and goes ahead with the flag", () => {
    const ci = { CI: "true" };
    expect(() => resolveApproval(LEAD, "env", LOGIN, ci, "linux")).toThrow(
      expect.objectContaining({ code: ExitCode.AUTH })
    );
    expect(resolveApproval(LEAD, "cli", LOGIN, ci, "linux")).toEqual({
      noBrowser: true,
      approver: "lead@example.com"
    });
  });
});
