/**
 * `oxyc api` against the one path prefix that is NOT put under `/api`.
 *
 * `customer-apps/…` is sent as written because `/customer-apps/<org>/<app>/…`
 * is where an app's bundle is served. The app registry is a different mount,
 * `/api/customer-apps/…`, so `oxyc api customer-apps` is a 404 — and the
 * generic 404 hint ("check the path with `oxyc routes`") points at a listing
 * that shows the path the caller believes they typed.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode } from "../util/errors.js";
import { type ApiFlags, runApi } from "./api.js";

const TARGET = "https://oxy.test";
const FLAGS: ApiFlags = { rawField: [], field: [], header: [] };

function fakeContext(): Context {
  return {
    cwd: "/tmp",
    flags: { env: "production" },
    target: () => TARGET,
    env: () => ({ target: TARGET, orgSlug: undefined }) as ReturnType<Context["env"]>,
    bearer: async () => "tok",
    maybeBearer: async () => "tok",
    storedBearer: () => "tok",
    async credential() {
      return { token: await this.bearer(), source: "env" };
    },
    serviceAccount: () => undefined,
    apiKey: () => undefined,
    customer: () => undefined,
    repoDir: () => undefined,
    placeholders: () => ({}),
    withEnv: () => fakeContext()
  };
}

/** Answer every request with `status`, and record the paths asked for. */
function stubStatus(status: number, body: unknown = { error: "nope" }): string[] {
  const paths: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string | URL) => {
      paths.push(String(input).replace(TARGET, ""));
      return new Response(JSON.stringify(body), { status });
    })
  );
  return paths;
}

async function failure(run: Promise<void>): Promise<CliError> {
  const cause = await run.then(
    () => undefined,
    (e: unknown) => e
  );
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("oxyc api customer-apps…", () => {
  it("still sends the path as written — the bundle mount is not under /api", async () => {
    const paths = stubStatus(404);
    await failure(runApi(fakeContext(), "customer-apps/acme/store/assets/main.js", FLAGS));
    expect(paths).toEqual(["/customer-apps/acme/store/assets/main.js"]);
  });

  it("names the /api spelling when the bundle mount answers 404", async () => {
    stubStatus(404);
    for (const [typed, suggested] of [
      ["customer-apps", "oxyc api api/customer-apps"],
      ["/customer-apps/some-id/builds", "oxyc api api/customer-apps/some-id/builds"],
      ["customer-apps/fleet-health", "oxyc api api/customer-apps/fleet-health"]
    ] as const) {
      const error = await failure(runApi(fakeContext(), typed, FLAGS));
      expect(error.code).toBe(ExitCode.NOT_FOUND);
      expect(error.hint).toContain(`\`${suggested}\``);
    }
  });

  it("gives the same hint when the 404 comes back through --paginate", async () => {
    stubStatus(404);
    const error = await failure(
      runApi(fakeContext(), "customer-apps", { ...FLAGS, paginate: true })
    );
    expect(error.code).toBe(ExitCode.NOT_FOUND);
    expect(error.hint).toContain("`oxyc api api/customer-apps`");
  });

  it("leaves every other failure alone", async () => {
    // A 404 under /api keeps the generic hint: the caller did spell it out.
    stubStatus(404);
    const underApi = await failure(runApi(fakeContext(), "api/customer-apps/nope", FLAGS));
    expect(underApi.hint).toContain("oxyc routes");

    // A prefix that merely starts with the same letters is not the mount.
    const lookalike = await failure(runApi(fakeContext(), "customer-apps-x", FLAGS));
    expect(lookalike.hint).toContain("oxyc routes");

    // And a non-404 on the mount is not a wrong-path problem at all.
    stubStatus(500);
    const broken = await failure(runApi(fakeContext(), "customer-apps", FLAGS));
    expect(broken.code).toBe(ExitCode.UNAVAILABLE);
    expect(broken.hint).toBeUndefined();
  });
});
