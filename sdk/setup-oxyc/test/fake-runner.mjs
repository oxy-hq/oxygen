/**
 * A GitHub Actions runner, faked: an `Io` whose network is a route table and
 * whose files, log lines and spawned commands are arrays a test can read.
 *
 * Test support only — `action.yml` never loads this file.
 */

/** @typedef {import("../src/io.mjs").Io} Io */
/** @typedef {import("../src/io.mjs").ExecResult} ExecResult */

/**
 * @typedef {object} Request
 * @property {string} method
 * @property {string} url
 * @property {Record<string, string>} headers Lower-cased names.
 * @property {string} [body]
 */

/** @typedef {{ status: number; body?: unknown; headers?: Record<string, string> }} Reply */

/**
 * Keyed `METHOD url`. `nth` is 1 for the first call to that route. Return an
 * `Error` to have the request fail the way a dead network does.
 *
 * @typedef {Record<string, (request: Request, nth: number) => Reply | Error>} Routes
 */

export const HOST = "https://oxy.test";
export const EXCHANGE = `POST ${HOST}/api/auth/oidc/exchange`;
export const REVOKE = `DELETE ${HOST}/api/auth/token`;
/**
 * GitHub's id-token endpoint, with the audience the action must ask for:
 * `oxy:<host>` of the deployment it is about to post to — never plain `oxy`.
 */
export const GITHUB_ID_TOKEN = "GET https://gh.test/token?api-version=2.0&audience=oxy%3Aoxy.test";

export const MINTED = {
  token: "oxy_ci_minted_secret",
  token_id: "tok-1",
  expires_at: "2026-10-01T12:15:00Z",
  service_account: "acme/deployer",
  grants: []
};

/** GitHub minting a distinct id token on every call, as it does. */
export const GITHUB_OK = {
  /** @type {Routes[string]} */
  [GITHUB_ID_TOKEN]: (_request, nth) => ({ status: 200, body: { value: `gh-jwt-${nth}` } })
};

/**
 * @param {object} [options]
 * @param {Record<string, string | undefined>} [options.env] Merged over a job
 *   granted `id-token: write`; `undefined` removes a variable.
 * @param {Routes} [options.routes] Anything unlisted answers 404.
 * @param {(command: string, args: string[]) => ExecResult} [options.exec]
 */
export function fakeRunner({ env = {}, routes = {}, exec } = {}) {
  /** @type {string[]} */
  const lines = [];
  /** @type {Record<string, string>} */
  const files = {};
  /** @type {Request[]} */
  const requests = [];
  /** @type {string[][]} */
  const commands = [];
  /** @type {number[]} */
  const sleeps = [];
  /** @type {Record<string, number>} */
  const counts = {};
  /** Log lines and file appends, in the order they happened. @type {string[]} */
  const events = [];

  /** @type {NodeJS.ProcessEnv} */
  const merged = {
    RUNNER_TEMP: "/runner/temp",
    GITHUB_ENV: "/files/env",
    GITHUB_PATH: "/files/path",
    GITHUB_OUTPUT: "/files/output",
    GITHUB_STATE: "/files/state",
    ACTIONS_ID_TOKEN_REQUEST_URL: "https://gh.test/token?api-version=2.0",
    ACTIONS_ID_TOKEN_REQUEST_TOKEN: "gh-request-token",
    ...env
  };
  for (const [name, value] of Object.entries(merged)) {
    if (value === undefined) delete merged[name];
  }

  /** @type {Io} */
  const io = {
    env: merged,
    fetch: async (input, init = {}) => {
      const method = (init.method ?? "GET").toUpperCase();
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      /** @type {Record<string, string>} */
      const headers = {};
      new Headers(init.headers).forEach((value, name) => {
        headers[name] = value;
      });
      const request = {
        method,
        url,
        headers,
        body: typeof init.body === "string" ? init.body : undefined
      };
      requests.push(request);
      const key = `${method} ${url}`;
      const nth = (counts[key] ?? 0) + 1;
      counts[key] = nth;
      const route = routes[key];
      if (!route) return new Response(JSON.stringify({ error: "not found" }), { status: 404 });
      const reply = route(request, nth);
      if (reply instanceof Error) throw reply;
      // A 204 may not carry a body — `Response` throws if it is given one.
      if (reply.status === 204) return new Response(null, { status: 204, headers: reply.headers });
      return new Response(JSON.stringify(reply.body ?? {}), {
        status: reply.status,
        headers: { "content-type": "application/json", ...reply.headers }
      });
    },
    write: (line) => {
      lines.push(line);
      events.push(`write ${line}`);
    },
    append: (path, text) => {
      files[path] = (files[path] ?? "") + text;
      events.push(`append ${path} ${text}`);
    },
    exec: (command, args) => {
      commands.push([command, ...args]);
      if (exec) return exec(command, args);
      return { status: 0, stdout: command.endsWith("oxyc") ? "0.6.0\n" : "", stderr: "" };
    },
    sleep: async (ms) => {
      sleeps.push(ms);
    }
  };

  return { io, lines, files, requests, commands, sleeps, events };
}

/**
 * Parse GitHub's `name<<delimiter / value / delimiter` file-command blocks.
 *
 * @param {string | undefined} text
 * @returns {Record<string, string>}
 */
export function assignments(text) {
  /** @type {Record<string, string>} */
  const found = {};
  const pattern = /^(.+?)<<(ghadelimiter_[0-9a-f-]+)\n([\s\S]*?)\n\2\n/gm;
  for (const match of (text ?? "").matchAll(pattern)) {
    const [, name = "", , value = ""] = match;
    found[name] = value;
  }
  return found;
}

/**
 * Run `action` and return what it threw.
 *
 * @param {() => Promise<unknown>} action
 * @returns {Promise<import("../src/oxy.mjs").SetupError>}
 */
export async function failure(action) {
  try {
    await action();
  } catch (cause) {
    return /** @type {import("../src/oxy.mjs").SetupError} */ (cause);
  }
  throw new Error("expected the step to fail, and it did not");
}
