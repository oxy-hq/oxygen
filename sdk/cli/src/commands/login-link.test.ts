/**
 * `oxyc login-link`.
 *
 * Three properties are worth pinning, and each is a way the command could be
 * wrong while still printing a URL. STDOUT IS THE URL AND NOTHING ELSE, because
 * it is read by `$(…)`. `--next` AND THE TICKET STAY IN THE FRAGMENT, because a
 * query string is in every access log. And A REFUSAL NAMES THE NEXT STEP FOR
 * THE CREDENTIAL THAT WAS ACTUALLY USED — "run `oxyc login`" is wrong advice
 * for a token sitting in `OXY_TOKEN`, which the login cache never overrides.
 *
 * The server route is stubbed: it is the contract being coded against, and an
 * older deployment is the stub with no route at all.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveCredential } from "../auth/credentials.js";
import { createContext } from "../context/resolve.js";
import { type Reply, stubFetch } from "../testing/stub-fetch.js";
import { CliError, ExitCode } from "../util/errors.js";
import { loginLinkUrl, parseNext, runLoginLink } from "./login-link.js";

const TARGET = "https://oxy.test";
const ROUTE = "POST /api/auth/browser-ticket";
const SECRET = "oxy_pat_secret";

/** Five minutes before `TICKET.expires_at`, so the expiry reads the same on every run. */
const NOW = new Date("2026-10-07T10:00:00Z");
const TICKET = {
  ticket: "tkt_9f3c2a",
  path: "/token-login#ticket=tkt_9f3c2a",
  expires_at: "2026-10-07T10:05:00Z",
  session_seconds: 28_800
};
const LINK = "https://oxy.test/token-login#ticket=tkt_9f3c2a";
const issued = (): Reply => ({ status: 200, body: TICKET });

let scratch: string;
let stdout: string;
let stderr: string;

const context = () => createContext({ env: "production", target: TARGET }, scratch);
/** What `oxyc login` left behind: the credential used when `OXY_TOKEN` is unset. */
const cache = (token: string) =>
  saveCredential(TARGET, { token, email: "ada@acme.test", is_app_admin: false });

async function refusal(run: Promise<unknown>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (thrown: unknown) => thrown
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

/** Everything a rendered error would put on the terminal. */
const rendered = (cause: CliError) =>
  [cause.message, cause.detail, cause.hint, cause.remedy].join("\n");

beforeEach(() => {
  scratch = mkdtempSync(join(tmpdir(), "oxyc-login-link-"));
  vi.stubEnv("OXY_CREDENTIALS_PATH", join(scratch, "credentials.json"));
  vi.stubEnv("OXY_TOKEN", SECRET);
  vi.stubEnv("OXYC_QUIET", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_URL", "");
  vi.stubEnv("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
  // Only the clock: the expiry is described against it, and a fixture built
  // from the real one reads "in 4 min" on the run that crosses a boundary.
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  stdout = "";
  stderr = "";
  vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
    stdout += String(chunk);
    return true;
  });
  vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
    stderr += String(chunk);
    return true;
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  rmSync(scratch, { recursive: true, force: true });
});

describe("parseNext", () => {
  it("takes a path on the deployment, its query and fragment included", () => {
    expect(parseNext(undefined)).toBeUndefined();
    for (const path of ["/", "/ide", "/a/store/orders?tab=open&page=2#top", "/a/two words"]) {
      expect(parseNext(path)).toBe(path);
    }
  });

  /**
   * Each of these is a way to name another origin without typing a scheme —
   * or, for the last three, a path a browser rewrites into one.
   */
  it("refuses anything that could leave the deployment, as a usage error", () => {
    for (const elsewhere of [
      "//evil.test/ide",
      "https://evil.test/ide",
      "evil.test/ide",
      "ide",
      "",
      "/\\evil.test",
      "\\\\evil.test",
      "/\t/evil.test",
      "/ide\n"
    ]) {
      let cause: unknown;
      try {
        parseNext(elsewhere);
      } catch (thrown) {
        cause = thrown;
      }
      expect(cause, JSON.stringify(elsewhere)).toBeInstanceOf(CliError);
      expect((cause as CliError).code, JSON.stringify(elsewhere)).toBe(ExitCode.USAGE);
    }
  });
});

describe("loginLinkUrl", () => {
  it("puts --next in the fragment beside the ticket, escaped whole", () => {
    const next = "/a/store/orders?tab=open&page=2#top";
    const url = new URL(loginLinkUrl(TARGET, TICKET.path, next));
    // Nothing in the part of the URL a server is sent.
    expect(url.pathname).toBe("/token-login");
    expect(url.search).toBe("");
    const fragment = new URLSearchParams(url.hash.slice(1));
    expect(fragment.get("ticket")).toBe(TICKET.ticket);
    expect(fragment.get("next")).toBe(next);
  });

  it("keeps it in a fragment even when the server's path has none", () => {
    expect(loginLinkUrl(TARGET, "/token-login", "/ide")).toBe(
      "https://oxy.test/token-login#next=%2Fide"
    );
  });

  it("keeps a target's own path prefix, whatever its trailing slash", () => {
    expect(loginLinkUrl("https://oxy.test/oxy/", TICKET.path)).toBe(
      "https://oxy.test/oxy/token-login#ticket=tkt_9f3c2a"
    );
  });
});

describe("oxyc login-link", () => {
  it("prints the URL and nothing else on stdout, asking with the bearer alone", async () => {
    const calls = stubFetch(TARGET, { [ROUTE]: issued });
    const opened: string[] = [];
    await runLoginLink(context(), {}, (url) => opened.push(url));
    expect(stdout).toBe(`${LINK}\n`);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.headers.authorization).toBe(`Bearer ${SECRET}`);
    expect(calls[0]?.body).toBeUndefined();
    // No --open, no browser.
    expect(opened).toEqual([]);
  });

  it("says on stderr what the link is, from the server's own numbers", async () => {
    stubFetch(TARGET, { [ROUTE]: issued });
    await runLoginLink(context());
    expect(stderr).toContain("works once");
    expect(stderr).toContain("in 5 min");
    expect(stderr).toContain("8 hours");
    expect(stderr).toContain("what this token can do and nothing more");
    expect(stderr).toContain("ends when the token is revoked");
  });

  it("states a session that is not a whole number of hours as it is", async () => {
    stubFetch(TARGET, {
      [ROUTE]: () => ({ status: 200, body: { ...TICKET, session_seconds: 5400 } })
    });
    await runLoginLink(context());
    expect(stderr).toContain("1.5 hours");
  });

  it("never prints the bearer, on either stream", async () => {
    stubFetch(TARGET, { [ROUTE]: issued });
    await runLoginLink(context(), { next: "/ide", open: true }, () => {});
    expect(`${stdout}${stderr}`).not.toContain(SECRET);
  });

  it("carries --next in the fragment and never sends it to the server", async () => {
    const calls = stubFetch(TARGET, { [ROUTE]: issued });
    await runLoginLink(context(), { next: "/ide?tab=files" });
    expect(stdout).toBe(`${LINK}&next=%2Fide%3Ftab%3Dfiles\n`);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.path).toBe("/api/auth/browser-ticket");
    expect(calls[0]?.body).toBeUndefined();
  });

  /**
   * With no credential at all: a wrong invocation answers "you called it
   * wrong" (2), not "log in" (4), and no ticket is spent on it.
   */
  it("refuses a --next off the deployment before a credential or a request", async () => {
    vi.stubEnv("OXY_TOKEN", "");
    const calls = stubFetch(TARGET, { [ROUTE]: issued });
    const cause = await refusal(runLoginLink(context(), { next: "//evil.test/ide" }));
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(calls).toHaveLength(0);
    expect(stdout).toBe("");
  });

  it("--json is one object: the url, and the server's own expiry and session length", async () => {
    stubFetch(TARGET, { [ROUTE]: issued });
    await runLoginLink(context(), { json: true, next: "/ide" });
    expect(stdout.trimEnd().split("\n")).toHaveLength(1);
    expect(JSON.parse(stdout)).toEqual({
      url: `${LINK}&next=%2Fide`,
      expires_at: TICKET.expires_at,
      session_seconds: TICKET.session_seconds
    });
  });

  it("--open opens the same URL it still prints", async () => {
    stubFetch(TARGET, { [ROUTE]: issued });
    const opened: string[] = [];
    await runLoginLink(context(), { open: true }, (url) => opened.push(url));
    expect(opened).toEqual([LINK]);
    expect(stdout).toBe(`${LINK}\n`);
    // The link works once, and the browser just took the once.
    expect(stderr).toContain("will not work again");
  });

  it("builds the link on the target's own path prefix", async () => {
    stubFetch("https://oxy.test/oxy", { [ROUTE]: issued });
    await runLoginLink(
      createContext({ env: "production", target: "https://oxy.test/oxy/" }, scratch)
    );
    expect(stdout).toBe("https://oxy.test/oxy/token-login#ticket=tkt_9f3c2a\n");
  });
});

describe("oxyc login-link, refused", () => {
  /**
   * 404 `no_token` is a session token asking — what a login cached before the
   * deployment minted tokens. It is the CREDENTIAL that is wrong, so the exit
   * code is the one that means "log in", whatever the status said.
   */
  it("reads no_token as a wrong credential, not a missing page: AUTH and the login to run", async () => {
    vi.stubEnv("OXY_TOKEN", "");
    cache("eyJ.session.jwt");
    stubFetch(TARGET, { [ROUTE]: () => ({ status: 404, body: { code: "no_token" } }) });
    const cause = await refusal(runLoginLink(context()));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("needs a personal access token");
    expect(cause.message).toContain("a session token");
    expect(cause.remedy).toBe(
      "run `oxyc login --env production`   (or put a personal access token in OXY_TOKEN)"
    );
    expect(cause.serverCode).toBe("no_token");
    expect(stdout).toBe("");
  });

  /**
   * `OXY_TOKEN` wins over the login cache, so "run `oxyc login`" alone would
   * be a browser flow that changes nothing. The variable comes first.
   */
  it("reads personal_token_required as AUTH, naming the credential and the variable it is in", async () => {
    vi.stubEnv("OXY_TOKEN", "oxy_sat_secret");
    stubFetch(TARGET, {
      [ROUTE]: () => ({ status: 403, body: { code: "personal_token_required" } })
    });
    const cause = await refusal(runLoginLink(context()));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("a service account token");
    expect(cause.remedy).toBe(
      "put a personal access token in OXY_TOKEN — or unset it and run `oxyc login --env production`"
    );
    expect(rendered(cause)).not.toContain("oxy_sat_secret");
    expect(stdout).toBe("");
  });

  /** A named variable is the only source: unsetting it leads to no login at all. */
  it("does not send a --token-env caller to a login the variable would never fall back to", async () => {
    vi.stubEnv("AGENT_TOKEN", "oxy_ci_secret");
    stubFetch(TARGET, {
      [ROUTE]: () => ({ status: 403, body: { code: "personal_token_required" } })
    });
    const ctx = createContext({ env: "dev", target: TARGET, tokenEnv: "AGENT_TOKEN" }, scratch);
    const cause = await refusal(runLoginLink(ctx));
    expect(cause.message).toContain("a CI token");
    expect(cause.remedy).toBe("put a personal access token in AGENT_TOKEN");
  });

  /** An unlisted route is the stub's 404 with no `code` — an older deployment exactly. */
  it("says a deployment without the route does not support sign-in links, not 'not found'", async () => {
    stubFetch(TARGET, {});
    const cause = await refusal(runLoginLink(context()));
    expect(cause.code).toBe(ExitCode.NOT_FOUND);
    expect(cause.message).toBe("https://oxy.test does not support sign-in links yet");
    expect(rendered(cause)).not.toMatch(/not found/i);
    expect(rendered(cause)).not.toContain("oxyc routes");
    expect(stdout).toBe("");
  });

  it("reads 401 as a credential that stopped working: AUTH and the login to run", async () => {
    vi.stubEnv("OXY_TOKEN", "");
    cache("oxy_pat_expired");
    stubFetch(TARGET, { [ROUTE]: () => ({ status: 401, body: { error: "unauthorized" } }) });
    const cause = await refusal(runLoginLink(context()));
    expect(cause.code).toBe(ExitCode.AUTH);
    expect(cause.message).toContain("no longer accepted");
    expect(cause.remedy).toContain("oxyc login --env production");
  });

  it("leaves any other refusal as the server's own, on the usual exit code", async () => {
    stubFetch(TARGET, { [ROUTE]: () => ({ status: 403, body: { error: "forbidden" } }) });
    const forbidden = await refusal(runLoginLink(context()));
    expect(forbidden.code).toBe(ExitCode.AUTH);
    expect(forbidden.message).not.toContain("personal access token");

    stubFetch(TARGET, { [ROUTE]: () => ({ status: 503, text: "upstream unavailable" }) });
    expect((await refusal(runLoginLink(context()))).code).toBe(ExitCode.UNAVAILABLE);

    stubFetch(TARGET, { [ROUTE]: () => ({ status: 404, body: { code: "org_not_found" } }) });
    const other = await refusal(runLoginLink(context()));
    expect(other.code).toBe(ExitCode.NOT_FOUND);
    expect(other.message).not.toContain("sign-in links");
  });

  /**
   * Exit 4 under a sandbox agent token means "stop and report", so it is
   * never told to log in — and it is refused before the request, whose 404
   * would otherwise read as a deployment with no such route.
   */
  it("refuses a sandbox agent token before any request, as a usage error", async () => {
    vi.stubEnv("OXY_TOKEN", `oxy_sbx_${"a".repeat(36)}`);
    const calls = stubFetch(TARGET, { [ROUTE]: issued });
    const cause = await refusal(runLoginLink(context()));
    expect(cause.code).toBe(ExitCode.USAGE);
    expect(cause.message).toContain("a sandbox agent token cannot open a browser session");
    expect(rendered(cause)).not.toContain("oxyc login");
    expect(calls).toHaveLength(0);
  });

  /**
   * The path is appended to the target and handed to a browser, and stdout is
   * one word. A deployment that answered any of these would otherwise have
   * this print a link to another host, or two lines.
   */
  it("prints nothing for an answer whose path could leave the deployment or split stdout", async () => {
    for (const path of [
      "@evil.test/token-login#ticket=t",
      ".evil.test/token-login#ticket=t",
      "//evil.test/token-login#ticket=t",
      "/\\evil.test/token-login#ticket=t",
      "/token-login#ticket=t\nsecond-line",
      "/token login#ticket=t",
      undefined
    ]) {
      stubFetch(TARGET, { [ROUTE]: () => ({ status: 200, body: { ...TICKET, path } }) });
      const cause = await refusal(runLoginLink(context()));
      expect(cause.code, JSON.stringify(path)).toBe(ExitCode.FAILURE);
      expect(cause.message).toContain("without a usable sign-in link");
    }
    expect(stdout).toBe("");
  });

  it("prints nothing for an answer missing its expiry or session length, or not JSON at all", async () => {
    for (const reply of [
      { status: 200, body: { ...TICKET, expires_at: undefined } },
      { status: 200, body: { ...TICKET, expires_at: "soon" } },
      { status: 200, body: { ...TICKET, session_seconds: "28800" } },
      { status: 200, body: { ...TICKET, session_seconds: 0 } },
      { status: 200, text: "<!doctype html><title>Oxygen</title>" }
    ] satisfies Reply[]) {
      stubFetch(TARGET, { [ROUTE]: () => reply });
      const cause = await refusal(runLoginLink(context()));
      expect(cause.code, JSON.stringify(reply)).toBe(ExitCode.FAILURE);
    }
    expect(stdout).toBe("");
  });
});
