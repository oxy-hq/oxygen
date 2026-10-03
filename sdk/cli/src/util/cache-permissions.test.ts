/**
 * Every cache this CLI writes must be owner-only.
 *
 * ONE TEST FOR ALL FOUR, and that is the point. Two of them were tightened in
 * one round and the other two were not, and nothing anywhere could see the
 * asymmetry: each cache decides its own mode, in its own file, next to its own
 * reasoning. A per-file test would have passed on both halves.
 *
 * What they hold: response bodies of authenticated multi-tenant requests, the
 * route table of a deployment, the customer list of the business read out of a
 * private GitHub org, and a map of every checkout on the machine. The response
 * cache already hashes the token into its key precisely so two users on one
 * box cannot read each other's — leaving the bytes world-readable hands back
 * exactly what that key protects.
 */

import { spawn } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync
} from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");
const SCRATCH: string[] = [];

afterAll(() => {
  for (const dir of SCRATCH) rmSync(dir, { recursive: true, force: true });
});

function scratch(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-perm-"));
  SCRATCH.push(dir);
  return dir;
}

/** `0o600`, `0o700`, … for a path. */
function mode(path: string): number {
  return statSync(path).mode & 0o777;
}

/**
 * Run the CLI with its caches pointed somewhere disposable.
 *
 * ASYNC, and that is not a style preference. `spawnSync` blocks this process's
 * event loop, so the loopback server below — which lives in this process —
 * could never answer the child, and every request sat until the 2-minute
 * client timeout. The first version of this file took 122 seconds to fail.
 */
function run(
  cacheDir: string,
  args: string[],
  pathPrefix?: string
): Promise<{ status: number; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [BIN, ...args], {
      env: {
        ...process.env,
        PATH: pathPrefix ? `${pathPrefix}:${process.env.PATH ?? ""}` : process.env.PATH,
        OXYC_CACHE_DIR: cacheDir,
        OXY_CREDENTIALS_PATH: join(cacheDir, "__no_creds__.json"),
        // `repos` otherwise scans the real home directory for checkouts and
        // runs `git remote get-url` in each one: seconds on a developer's
        // machine (past the 5s test timeout under load), nothing on CI's empty
        // home. A root that does not exist makes both the same, and the scan
        // still writes `repos.json`, whose mode is what is asserted here.
        OXYC_REPO_ROOTS: join(cacheDir, "__no_repo_roots__"),
        OXY_TOKEN: "cache-permissions-test",
        NO_COLOR: "1"
      }
    });
    let stderr = "";
    child.stderr.on("data", (d) => {
      stderr += d;
    });
    child.stdout.resume();
    child.on("close", (status) => resolve({ status: status ?? -1, stderr }));
  });
}

/**
 * A directory holding a fake `gh`, to be put in front of `PATH`.
 *
 * The customers cache is only written on a SUCCESSFUL listing, which never
 * happens in CI — so pointing the CLI at an unauthenticated `gh` covered the
 * `customers/` directory and never the file inside it, and reverting its
 * `{mode: 0o600}` passed. This stub answers the two calls the listing makes
 * (`gh auth status`, then `gh repo list … --json name,description`) so the
 * real write path runs with no network and no credential anywhere.
 */
function ghStub(): string {
  const dir = mkdtempSync(join(tmpdir(), "oxyc-ghstub-"));
  SCRATCH.push(dir);
  const path = join(dir, "gh");
  writeFileSync(
    path,
    [
      "#!/bin/sh",
      "# `gh auth status` writes to stderr and exits 0 when authenticated.",
      'if [ "$1" = "auth" ]; then echo "Logged in as stub" >&2; exit 0; fi',
      'if [ "$1" = "repo" ] && [ "$2" = "list" ]; then',
      '  echo \'[{"name":"stub-customer","description":"a customer, for the cache"}]\'',
      "  exit 0",
      "fi",
      'echo "gh stub: unexpected $*" >&2',
      "exit 1",
      ""
    ].join("\n")
  );
  chmodSync(path, 0o755);
  return dir;
}

describe("cache file permissions", () => {
  let server: Server;
  let base: string;

  beforeAll(async () => {
    // A real server, because the modes worth asserting are on files the CLI
    // only writes when a request SUCCEEDS. Against an unreachable host the
    // auth check throws first and nothing is written at all — a test built
    // that way asserts the absence of a file and passes on any mode.
    server = createServer((_req, res) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true }));
    });
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });

  afterAll(() => server.close());

  it("writes response-cache entries 0600 inside a 0700 directory", async () => {
    const dir = scratch();
    const r = await run(dir, ["api", "user", "--target", base, "--cache", "5m"]);
    expect(r.status, r.stderr).toBe(0);

    const responses = join(dir, "responses");
    expect(existsSync(responses), "the response cache directory was never created").toBe(true);
    expect(mode(responses)).toBe(0o700);

    const entries = readdirSync(responses);
    expect(entries.length, "the response was not cached").toBeGreaterThan(0);
    for (const entry of entries) expect(mode(join(responses, entry))).toBe(0o600);
  });

  it("closes every directory and file it creates under the cache root", async () => {
    const dir = scratch();
    await run(dir, ["api", "user", "--target", base, "--cache", "5m"]);
    await run(dir, ["repos"]);
    // The CUSTOMERS cache too, through a STUBBED `gh` — see `ghStub`. The walk
    // below only inspects what exists, so a run that never wrote
    // `customers/<org>.json` asserted nothing about the file, and reverting its
    // 0600 passed while the test read as covering four caches.
    await run(dir, ["list"], ghStub());

    // THE STUB REALLY RAN. Without this the walk below is satisfied by a
    // `customers/` directory the read path created and an absent file inside
    // it — which is exactly the state that let a world-readable customer list
    // pass as covered.
    const customers = join(dir, "customers");
    expect(existsSync(customers), "the customers cache directory was never created").toBe(true);
    expect(
      readdirSync(customers).length,
      "the customer listing wrote no cache file — the gh stub did not answer"
    ).toBeGreaterThan(0);

    const offenders: string[] = [];
    const walk = (path: string) => {
      for (const entry of readdirSync(path, { withFileTypes: true })) {
        const full = join(path, entry.name);
        const m = mode(full);
        const want = entry.isDirectory() ? 0o700 : 0o600;
        if (m !== want) offenders.push(`${full} is ${m.toString(8)}, want ${want.toString(8)}`);
        if (entry.isDirectory()) walk(full);
      }
    };
    walk(dir);
    expect(offenders, offenders.join("\n")).toEqual([]);

    // …and it inspected something. A walk over an empty directory finds no
    // offenders either, which is how this test claimed four caches while
    // covering two.
    let inspected = 0;
    const count = (path: string) => {
      for (const entry of readdirSync(path, { withFileTypes: true })) {
        inspected += 1;
        if (entry.isDirectory()) count(join(path, entry.name));
      }
    };
    count(dir);
    expect(inspected, "the walk found nothing to check").toBeGreaterThan(1);
  });

  /**
   * `repos.json` sits directly in the cache ROOT rather than in a
   * subdirectory, so the per-subdirectory 0700s do not cover it — the root
   * itself has to be closed.
   */
  it("closes the cache root, because repos.json is written at the top level", async () => {
    const dir = scratch();
    await run(dir, ["repos"]);
    expect(existsSync(join(dir, "repos.json")), "the repo scan wrote no cache").toBe(true);
    expect(mode(join(dir, "repos.json"))).toBe(0o600);
    expect(mode(dir)).toBe(0o700);
  });
});
