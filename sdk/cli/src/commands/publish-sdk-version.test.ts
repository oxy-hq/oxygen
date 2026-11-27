/**
 * `oxyc publish` sends `sdk_version`, for the design-system check's SDK floor:
 * the built binary against a fake deployment, asserting on the upload's fields.
 *
 * The fake answers the two routes a token publish of a pre-built bundle calls.
 * Everything else about a publish is in `publish.test.ts`, whose fake answers
 * the rest.
 */

import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLI_KILL_TIMEOUT_MS,
  CLI_TEST_TIMEOUT_MS,
  cliRunError,
  reapChildrenAfterEach
} from "../testing/cli-child.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");

// Every case here runs the built CLI: see `testing/cli-child.ts` for the budget and the kill timeout.
vi.setConfig({ testTimeout: CLI_TEST_TIMEOUT_MS });
const track = reapChildrenAfterEach();

/** One upload the fake received: its text fields. */
interface Upload {
  fields: Record<string, string>;
}

let server: Server;
let target: string;
let received: Upload[] = [];

/** The text fields of a multipart upload. */
async function fieldsOf(req: IncomingMessage): Promise<Record<string, string>> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  const form = await new Request("http://x", {
    method: "POST",
    headers: { "content-type": req.headers["content-type"] ?? "" },
    body: Buffer.concat(chunks)
  }).formData();
  const fields: Record<string, string> = {};
  for (const [name, value] of form.entries()) {
    if (typeof value === "string") fields[name] = value;
  }
  return fields;
}

beforeAll(async () => {
  // oxlint-disable-next-line typescript/no-misused-promises -- a throw in this stub is an unhandled rejection, which vitest reports as a failure
  server = createServer(async (req, res) => {
    const path = req.url ?? "/";
    const reply = (status: number, value: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(value));
    };
    if (path === "/api/apps/acme/sales/build-config") return reply(200, { project_id: "proj-1" });
    if (path !== "/api/customer-apps/publish") return reply(404, { error: `unexpected ${path}` });
    const fields = await fieldsOf(req);
    received.push({ fields });
    if (req.headers.authorization !== "Bearer good-token") {
      return reply(403, { error: "not an app admin" });
    }
    reply(200, {
      app_id: "app-1",
      build_id: fields.build_id,
      url: "/customer-apps/acme/sales/",
      channel: fields.channel,
      org_slug: "acme",
      is_new_app: false
    });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

let work: string;
beforeEach(() => {
  work = mkdtempSync(join(tmpdir(), "oxyc-publish-sdk-"));
  received = [];
});
afterEach(() => rmSync(work, { recursive: true, force: true }));

/** An app directory with a manifest and a pre-built `out/`. */
function app(): string {
  const dir = join(work, "app");
  mkdirSync(join(dir, "out"), { recursive: true });
  writeFileSync(
    join(dir, "oxy-app.json"),
    JSON.stringify({ schemaVersion: 2, slug: "sales", orgSlug: "acme" })
  );
  writeFileSync(join(dir, "out", "index.html"), "<html>sales</html>");
  return dir;
}

/**
 * The binary, with an environment built from nothing: a runner's own
 * `GITHUB_*` / `ACTIONS_*` would otherwise leak into the build id and the auth
 * decision, and the real credentials file must never be read.
 */
async function publish(
  cwd: string,
  args: string[],
  env: Record<string, string> = {}
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  const argv = ["publish", "--target", target, ...args];
  const child = track(
    spawn(process.execPath, [BIN, ...argv], {
      cwd,
      env: {
        PATH: process.env.PATH ?? "",
        HOME: work,
        OXY_CREDENTIALS_PATH: join(work, "credentials.json"),
        NO_COLOR: "1",
        ...env
      },
      timeout: CLI_KILL_TIMEOUT_MS
    })
  );
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (d) => {
    stdout += d;
  });
  child.stderr.on("data", (d) => {
    stderr += d;
  });
  const status = await new Promise<number | null>((done, fail) => {
    child.on("error", (e) => fail(cliRunError(argv, e.message)));
    child.on("close", (code, signal) => (signal ? fail(cliRunError(argv, signal)) : done(code)));
  });
  return { status, stdout, stderr };
}

function uploads(): Upload[] {
  return received;
}

describe("oxyc publish: sdk_version, for the design-system check's SDK floor", () => {
  /** The app's `package.json`, and optionally an installed copy of the SDK. */
  function dependsOnSdk(dir: string, range: string, installed?: string): void {
    writeFileSync(
      join(dir, "package.json"),
      JSON.stringify({ name: "sales", dependencies: { "@oxy-hq/sdk": range } })
    );
    if (installed === undefined) return;
    const pkg = join(dir, "node_modules", "@oxy-hq", "sdk");
    mkdirSync(pkg, { recursive: true });
    writeFileSync(
      join(pkg, "package.json"),
      JSON.stringify({ name: "@oxy-hq/sdk", version: installed })
    );
  }

  it("sends the installed SDK's version over the declared range", async () => {
    const dir = app();
    dependsOnSdk(dir, "^2.30.0", "2.31.4");
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status, result.stderr).toBe(0);
    expect(uploads()[0]?.fields.sdk_version).toBe("2.31.4");
  });

  it("sends the declared range's floor when nothing is installed", async () => {
    const dir = app();
    dependsOnSdk(dir, "^2.30.0");
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status, result.stderr).toBe(0);
    expect(uploads()[0]?.fields.sdk_version).toBe("2.30.0");
  });

  it("leaves the field out for an app that does not depend on the SDK", async () => {
    const dir = app();
    writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "sales" }));
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status, result.stderr).toBe(0);
    expect(uploads()[0]?.fields).toHaveProperty("build_id");
    expect(uploads()[0]?.fields).not.toHaveProperty("sdk_version");
  });
});
