/**
 * What a `401`/`403` is told when the bearer is a token in the variable.
 *
 * The generic hint says "try `oxyc login` again". Under an agent's own token
 * that is the one wrong move, and for any token in the variable it fixes
 * nothing. So the swap is pinned here, with what it must leave alone.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type ApiResponse, AUTH_HINT, errorForResponse } from "../api/request.js";
import { createContext } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import {
  forgetVariableCredential,
  hintToShow,
  isAgentTokenRow,
  noteVariableCredential
} from "./agent-token.js";
import { saveCredential } from "./credentials.js";

const TARGET = "https://oxy.test";
const AGENT_TOKEN = "oxy_pat_0123456789abcdefghijABCDEFGHIJ012345";

/** The error `oxyc api` raises for a refused request: the generic hint and all. */
const refusedRequest = (status = 401) => {
  const response: ApiResponse = {
    status,
    statusText: status === 401 ? "Unauthorized" : "Forbidden",
    url: `${TARGET}/api/orgs`,
    body: '{"error":"nope"}',
    headers: {},
    fromCache: false
  };
  return errorForResponse(response);
};

let scratch: string;

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-agent-token-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", "");
  forgetVariableCredential();
});

afterEach(() => {
  forgetVariableCredential();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("isAgentTokenRow", () => {
  it("is a personal token whose source is the agent approval, and nothing else", () => {
    expect(isAgentTokenRow({ kind: "personal", source: "oxyc_agent" })).toBe(true);
    expect(isAgentTokenRow({ kind: "personal", source: "oxyc_login" })).toBe(false);
    expect(isAgentTokenRow({ kind: "personal", source: "ui" })).toBe(false);
    expect(isAgentTokenRow({ kind: "personal" })).toBe(false);
    expect(isAgentTokenRow({ kind: "sandbox_agent", source: "oxyc_agent" })).toBe(false);
    expect(isAgentTokenRow({ source: "oxyc_agent" })).toBe(false);
  });
});

describe("the hint a refused request is shown with", () => {
  it("is the generic one, naming a login, when nothing says where the bearer came from", () => {
    const cause = refusedRequest();
    expect(cause.hint).toBe(AUTH_HINT);
    expect(hintToShow(cause)).toBe(AUTH_HINT);
    expect(AUTH_HINT).toContain("try `oxyc login` again");
  });

  it.each([401, 403])(
    "tells the holder of a token in the variable to stop and report, never to log in (%i)",
    (status) => {
      noteVariableCredential("OXY_TOKEN", AGENT_TOKEN);
      const shown = hintToShow(refusedRequest(status)) ?? "";
      expect(shown).not.toContain("try `oxyc login` again");
      expect(shown).toContain("a login cannot fix that: OXY_TOKEN wins over the login cache");
      expect(shown).toContain("an agent: stop and report this to your operator");
      expect(shown).toContain("do not look for another credential, a cached `oxyc login` included");
      // What is still true of a 403 on a tenant surface is kept.
      expect(shown).toContain("oxyc assume <org> --reason");
      expect(shown).not.toContain(AGENT_TOKEN);
    }
  );

  it("names the variable the bearer was read from", () => {
    noteVariableCredential("AGENT_TOKEN", AGENT_TOKEN);
    expect(hintToShow(refusedRequest())).toContain("the token in AGENT_TOKEN may have expired");
  });

  it("leaves every hint a command wrote itself alone", () => {
    noteVariableCredential("OXY_TOKEN", AGENT_TOKEN);
    const own = new CliError("not allowed", { code: ExitCode.AUTH, hint: "ask an org admin" });
    expect(hintToShow(own)).toBe("ask an org admin");
    const none = new CliError("not allowed", { code: ExitCode.AUTH });
    expect(hintToShow(none)).toBeUndefined();
    // The same sentence on another exit code is not this case.
    const other = new CliError("odd", { code: ExitCode.NOT_FOUND, hint: AUTH_HINT });
    expect(hintToShow(other)).toBe(AUTH_HINT);
  });

  it("swaps nothing for a credential that is not a personal access token", () => {
    for (const token of [
      "oxy_sat_0123456789abcdefghijABCDEFGHIJ012345",
      "oxy_ci_0123456789abcdefghijABCDEFGHIJ012345",
      "oxy_0123456789abcdef0123456789abcdef",
      "eyJ.session.jwt"
    ]) {
      noteVariableCredential("OXY_TOKEN", token);
      expect(hintToShow(refusedRequest()), token).toBe(AUTH_HINT);
    }
  });
});

describe("where the bearer came from is noted as the credential is resolved", () => {
  const context = (flags: { tokenEnv?: string } = {}) =>
    createContext({ env: "production", target: TARGET, ...flags }, scratch);

  it("notes a token read from OXY_TOKEN", async () => {
    vi.stubEnv("OXY_TOKEN", AGENT_TOKEN);
    await context().credential();
    expect(hintToShow(refusedRequest())).toContain("the token in OXY_TOKEN");
  });

  it("notes the variable --token-env named", async () => {
    vi.stubEnv("MY_AGENT", AGENT_TOKEN);
    await context({ tokenEnv: "MY_AGENT" }).credential();
    expect(hintToShow(refusedRequest())).toContain("the token in MY_AGENT");
  });

  it("notes nothing for a cached login: a person is still told to log in again", async () => {
    saveCredential(TARGET, { token: AGENT_TOKEN, email: "ada@acme.test", is_app_admin: false });
    const credential = await context().credential();
    expect(credential.source).toBe("file");
    expect(hintToShow(refusedRequest())).toBe(AUTH_HINT);
  });
});
