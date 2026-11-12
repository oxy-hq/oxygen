/**
 * A transport failure names its cause. Node's fetch throws "fetch failed" for
 * a refused connection and an untrusted certificate alike; the live cloud test
 * on #4061 hit SELF_SIGNED_CERT_IN_CHAIN and oxyc said only "check the target
 * is reachable", which is the wrong fix for that one.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { CliError, ExitCode } from "../util/errors.js";
import { certificateHint, request, supportsSystemCa, transportReason } from "./request.js";

function failWith(code: string): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => {
      throw new TypeError("fetch failed", { cause: Object.assign(new Error(code), { code }) });
    })
  );
}

async function refusal(): Promise<CliError> {
  const cause = await request({
    target: "https://oxy.example",
    path: "/api/user",
    method: "GET",
    bearer: "oxy_pat_x"
  }).catch((e: unknown) => e);
  expect(cause).toBeInstanceOf(CliError);
  return cause as CliError;
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
});

describe("transportReason", () => {
  it("is the code under fetch's own error, and nothing else", () => {
    expect(transportReason({ cause: { code: "ECONNREFUSED" } })).toBe("ECONNREFUSED");
    expect(transportReason(new Error("fetch failed"))).toBeUndefined();
    expect(transportReason({ cause: { code: "not a code\u001b[31m" } })).toBeUndefined();
    expect(transportReason(null)).toBeUndefined();
  });
});

describe("a request that never reached the deployment", () => {
  it("names an untrusted certificate and how to trust the proxy's CA", async () => {
    vi.stubEnv("HTTPS_PROXY", "");
    failWith("SELF_SIGNED_CERT_IN_CHAIN");
    const cause = await refusal();
    expect(cause.code).toBe(ExitCode.UNAVAILABLE);
    expect(cause.message).toContain("(SELF_SIGNED_CERT_IN_CHAIN)");
    expect(cause.hint).toContain("NODE_EXTRA_CA_CERTS");
    expect(cause.hint).toBe(certificateHint(process.versions.node));
  });

  it("names a refused connection, and keeps the reachability hint for it", async () => {
    vi.stubEnv("HTTPS_PROXY", "");
    vi.stubEnv("HTTP_PROXY", "");
    vi.stubEnv("https_proxy", "");
    vi.stubEnv("http_proxy", "");
    failWith("ECONNREFUSED");
    const cause = await refusal();
    expect(cause.message).toContain("(ECONNREFUSED)");
    expect(cause.hint).toContain("check the target is reachable");
  });
});

describe("certificateHint", () => {
  it("offers --use-system-ca only on a Node that has it", () => {
    for (const v of ["20.19.0", "21.7.3", "22.14.0", "23.7.0"]) {
      expect(supportsSystemCa(v), v).toBe(false);
      expect(certificateHint(v)).not.toContain("--use-system-ca");
      expect(certificateHint(v)).toContain("NODE_EXTRA_CA_CERTS");
    }
    for (const v of ["22.15.0", "22.23.4", "23.8.0", "24.0.0", "26.7.0"]) {
      expect(supportsSystemCa(v), v).toBe(true);
      expect(certificateHint(v)).toContain("NODE_OPTIONS=--use-system-ca");
    }
  });
});
