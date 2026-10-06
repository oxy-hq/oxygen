/**
 * `oxyc publish`, end to end: the built binary against a fake deployment.
 *
 * The fake answers the three routes a publish calls plus the trusted-publishing
 * pair, and records what arrived, so each case asserts on the REQUEST — the
 * fields, the bundle's contents, which credential — rather than on the prose.
 */

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { SERVICE_ACCOUNT_ID } from "../testing/stub-fetch.js";

import { ExitCode } from "../util/errors.js";
import { inferOrgApp } from "./publish.js";

const BIN = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "dist", "main.mjs");

interface Received {
  path: string;
  method: string;
  authorization?: string;
  fields: Record<string, string>;
  bundle?: Buffer;
  exchangeBody?: string;
}

let server: Server;
let target: string;
let received: Received[] = [];
/** Stand in for a deployment predating `app_id` on the exchange response. */
let noAppId = false;
/** What `POST /api/auth/oidc/exchange` answers. Unset: the route does not exist. */
let generalExchange: { status: number; body: unknown } | undefined;
/** The app has no publisher registered the older way. */
let publisherRefuses = false;
/** What the fake answers the upload with. */
let uploadStatus = 200;
/** What the fake answers the project's database list with. */
let databasesStatus = 200;
const DATABASES = [
  { name: "ch", dialect: "clickhouse", db_type: "clickhouse", datasets: null, synced: false },
  { name: "ah", dialect: "duckdb", db_type: "airhouse_managed", datasets: null, synced: false }
];

async function body(req: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks);
}

beforeAll(async () => {
  // oxlint-disable-next-line typescript/no-misused-promises -- a throw in this stub is an unhandled rejection, which vitest reports as a failure
  server = createServer(async (req, res) => {
    const path = req.url ?? "/";
    const raw = await body(req);
    const record: Received = {
      path,
      method: req.method ?? "GET",
      authorization: req.headers.authorization,
      fields: {}
    };
    received.push(record);
    const reply = (status: number, value: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(value));
    };

    if (path === "/api/apps/acme/sales/build-config") return reply(200, { project_id: "proj-1" });
    if (path.startsWith("/api/apps/")) return reply(404, { error: "not found" });
    if (path === "/api/proj-1/databases") {
      return databasesStatus === 200 ? reply(200, DATABASES) : reply(databasesStatus, {});
    }
    if (path === "/api/org-for-project/proj-9") return reply(200, { org_slug: "acme" });
    // Staging branch compile: queued on the POST, ready on the first poll.
    if (path === "/api/proj-1/compile/staging?branch=feat%2Fviews" && req.method === "POST") {
      return reply(200, { git_sha: "abc123", status: "pending", revision_id: null });
    }
    if (path === "/api/proj-1/compile/staging/status?git_sha=abc123") {
      return reply(200, {
        git_sha: "abc123",
        status: "ready",
        revision_id: "7e57a9e0-1111-4222-8333-944455556666"
      });
    }
    if (path.startsWith("/github-oidc")) {
      if (req.headers.authorization !== "bearer gh-request-token") return reply(403, {});
      // One token per audience, as GitHub mints them: neither exchange accepts
      // the other's.
      if (path.includes("audience=oxy-publish")) return reply(200, { value: "gh-jwt" });
      // The general exchange's audience is `oxy:<host>` of THIS server — the
      // host the token is about to be posted to. Plain `oxy` is never minted.
      const ours = encodeURIComponent(`oxy:${new URL(target).host}`);
      if (path.endsWith(`audience=${ours}`)) return reply(200, { value: "gh-jwt-oxy" });
      return reply(403, {});
    }
    // The general exchange. Absent (404) unless a case installs an answer —
    // which is what a deployment predating trusted access looks like.
    if (path === "/api/auth/oidc/exchange") {
      record.exchangeBody = raw.toString();
      if (!generalExchange) return reply(404, { error: "not found" });
      return JSON.parse(record.exchangeBody).token === "gh-jwt-oxy"
        ? reply(generalExchange.status, generalExchange.body)
        : reply(401, { error: "bad audience", code: "wrong_audience" });
    }
    if (path === "/api/auth/token" && req.method === "DELETE") {
      res.writeHead(204);
      res.end();
      return;
    }
    if (path === "/api/customer-apps/publish/oidc-exchange") {
      record.exchangeBody = raw.toString();
      if (publisherRefuses) return reply(403, { error: "no publisher registered" });
      return req.headers.authorization === "Bearer gh-jwt"
        ? reply(
            200,
            noAppId
              ? { token: "minted", expires_at: "later" }
              : {
                  token: "minted",
                  expires_at: "later",
                  app_id: "0b0e5a10-1111-4222-8333-944455556666"
                }
          )
        : reply(401, { error: "invalid OIDC token" });
    }
    if (path === "/api/customer-apps/publish") {
      const form = await new Request("http://x", {
        method: "POST",
        headers: { "content-type": req.headers["content-type"] ?? "" },
        body: raw
      }).formData();
      for (const [name, value] of form.entries()) {
        if (typeof value === "string") record.fields[name] = value;
        else record.bundle = Buffer.from(await value.arrayBuffer());
      }
      const token = req.headers.authorization;
      if (
        token !== "Bearer good-token" &&
        token !== "Bearer minted" &&
        token !== "Bearer oxy_ci_minted"
      ) {
        return reply(403, { error: "not an app admin" });
      }
      if (uploadStatus !== 200) return reply(uploadStatus, { error: "nope" });
      const environment = record.fields.environment;
      return reply(200, {
        app_id: "app-1",
        build_id: record.fields.build_id,
        url: "/customer-apps/acme/sales/",
        channel: record.fields.channel,
        org_slug: "acme",
        is_new_app: false,
        warnings: ["schedule for fn refresh did not register"],
        ...(environment
          ? {
              environment,
              environment_url: `https://${environment}--acme--sales.customer-apps-dev.oxygen-hq.com/`
            }
          : {})
      });
    }
    reply(404, { error: `unexpected ${path}` });
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  target = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => {
  server.close();
});

let work: string;
beforeEach(() => {
  work = mkdtempSync(join(tmpdir(), "oxyc-publish-"));
  received = [];
  noAppId = false;
  generalExchange = undefined;
  publisherRefuses = false;
  uploadStatus = 200;
  databasesStatus = 200;
});
afterEach(() => rmSync(work, { recursive: true, force: true }));

/** An app directory with a manifest and a pre-built `out/`. */
function app(manifest: Record<string, unknown> = { slug: "sales", orgSlug: "acme" }): string {
  const dir = join(work, "app");
  mkdirSync(join(dir, "out"), { recursive: true });
  writeFileSync(join(dir, "oxy-app.json"), JSON.stringify(manifest));
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
  const child = spawn(process.execPath, [BIN, "publish", "--target", target, ...args], {
    cwd,
    env: {
      PATH: process.env.PATH ?? "",
      HOME: work,
      OXY_CREDENTIALS_PATH: join(work, "credentials.json"),
      NO_COLOR: "1",
      ...env
    }
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (d) => {
    stdout += d;
  });
  child.stderr.on("data", (d) => {
    stderr += d;
  });
  const status = await new Promise<number | null>((r) => child.on("close", r));
  return { status, stdout, stderr };
}

function uploads(): Received[] {
  return received.filter((r) => r.path === "/api/customer-apps/publish");
}

function tarList(bundle: Buffer): string[] {
  const file = join(work, "sent.tar.gz");
  writeFileSync(file, bundle);
  return spawnSync("tar", ["-tzf", file], { encoding: "utf8" }).stdout.split("\n").filter(Boolean);
}

describe("oxyc publish", () => {
  it("publishes a pre-built bundle as a draft with the fields the server reads", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--build-id", "b-1"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status, result.stderr).toBe(0);

    const [upload] = uploads();
    expect(upload?.authorization).toBe("Bearer good-token");
    expect(upload?.fields).toMatchObject({
      app: "sales",
      org: "acme",
      project: "proj-1",
      build_id: "b-1",
      channel: "draft"
    });
    expect(tarList(upload?.bundle ?? Buffer.alloc(0))).toEqual(["index.html"]);
    expect(result.stdout).toContain("Published new version of acme/sales (id app-1)");
    // The server's warnings, which the Rust client dropped.
    expect(result.stderr).toContain("schedule for fn refresh did not register");
  });

  it("prints the server's result as JSON on stdout with --json, and promotes", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--json", "--promote"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status, result.stderr).toBe(0);
    const parsed = JSON.parse(result.stdout) as { channel: string; warnings: string[] };
    expect(parsed.channel).toBe("published");
    expect(parsed.warnings).toHaveLength(1);
  });

  it("compiles --semantic-branch, waits for it, and pins the draft to the revision", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--semantic-branch", "feat/views"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status, result.stderr).toBe(0);
    const compile = received.find((r) => r.path.startsWith("/api/proj-1/compile/staging?"));
    expect(compile?.authorization).toBe("Bearer good-token");
    expect(uploads()[0]?.fields).toMatchObject({
      channel: "draft",
      semantic_revision_id: "7e57a9e0-1111-4222-8333-944455556666"
    });
    // `--branch` is the app source branch; the semantic branch must not leak into it.
    expect(uploads()[0]?.fields.branch).not.toBe("feat/views");
  });

  it("refuses --semantic-branch with --promote before calling the server", async () => {
    const dir = app();
    const result = await publish(
      dir,
      ["--dir", "out", "--promote", "--semantic-branch", "feat/views"],
      { OXY_TOKEN: "good-token" }
    );
    expect(result.status).toBe(ExitCode.USAGE);
    expect(result.stderr).toContain("staging-only");
    expect(received).toHaveLength(0);
  });

  it("--app-env sends the environment field, and prints environment_url", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--app-env", "dev-a1", "--json"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status, result.stderr).toBe(0);
    const [upload] = uploads();
    expect(upload?.fields.environment).toBe("dev-a1");
    const parsed = JSON.parse(result.stdout) as { environment: string; environment_url: string };
    expect(parsed.environment).toBe("dev-a1");
    expect(parsed.environment_url).toContain("dev-a1");
  });

  it("rejects a malformed --app-env before any request", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--app-env", "not-a-sandbox"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status).toBe(ExitCode.USAGE);
    expect(received).toHaveLength(0);
  });

  it("refuses --promote combined with --app-env, before calling the server", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--app-env", "dev-a1", "--promote"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status).toBe(ExitCode.USAGE);
    expect(result.stderr).toContain("--app-env");
    expect(received).toHaveLength(0);
  });

  it("refuses a publish token with --app-env, before calling the server", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out", "--app-env", "dev-a1"], {
      OXY_TOKEN: "oxypublish_stored"
    });
    expect(result.status).toBe(ExitCode.USAGE);
    expect(received).toHaveLength(0);
  });

  it("builds from source with the app's base path exported", async () => {
    const dir = app({
      slug: "sales",
      orgSlug: "acme",
      build: {
        install: "true",
        command: 'mkdir -p dist && printf %s "$OXY_APP_BASE_PATH" > dist/index.html',
        outDir: "dist"
      }
    });
    const result = await publish(dir, [], { OXY_TOKEN: "good-token" });
    expect(result.status, result.stderr).toBe(0);
    const file = join(work, "check.tar.gz");
    writeFileSync(file, uploads()[0]?.bundle ?? Buffer.alloc(0));
    const html = spawnSync("tar", ["-xzOf", file, "index.html"], { encoding: "utf8" }).stdout;
    expect(html).toBe("/customer-apps/acme/sales/");
  });

  /** Before the build: a missing login costs a second, not a two-minute install. */
  it("refuses without a credential before running the build", async () => {
    const dir = app({
      slug: "sales",
      orgSlug: "acme",
      build: { install: "touch installed", command: "true" }
    });
    const result = await publish(dir, []);
    expect(result.status).toBe(ExitCode.AUTH);
    expect(result.stderr).toContain("oxyc login");
    expect(existsSync(join(dir, "installed"))).toBe(false);
    expect(uploads()).toHaveLength(0);
  });

  /** Publishing past it used to ship the bundle with no functions and a guessed identity. */
  it("stops on a broken oxy-app.json before building or uploading", async () => {
    const dir = app();
    writeFileSync(join(dir, "oxy-app.json"), '{ "slug": "sales", "orgSlug": "acme", ');
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status).toBe(ExitCode.USAGE);
    expect(result.stderr).toContain("is not a valid oxy-app.json");
    expect(uploads()).toHaveLength(0);
  });

  it("maps a 403 to the auth exit code with the server's body", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "wrong-token" });
    expect(result.status).toBe(ExitCode.AUTH);
    expect(result.stderr).toContain("not an app admin");
    expect(result.stderr).toContain("oxyc whoami");
  });

  it("maps a duplicate build id to the request exit code", async () => {
    uploadStatus = 409;
    const dir = app();
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status).toBe(ExitCode.REQUEST);
  });

  it("names an unregistered app as not found", async () => {
    const dir = app({ slug: "other", orgSlug: "acme" });
    const result = await publish(dir, ["--dir", "out"], { OXY_TOKEN: "good-token" });
    expect(result.status).toBe(ExitCode.NOT_FOUND);
    expect(result.stderr).toContain("--project");
  });

  /** A pinned workspace determines its org, so the manifest need not name one. */
  it("resolves the org from a pinned project", async () => {
    const dir = app({ slug: "sales" });
    const result = await publish(dir, ["--dir", "out", "--project", "proj-9"], {
      OXY_TOKEN: "good-token"
    });
    expect(result.status, result.stderr).toBe(0);
    expect(uploads()[0]?.fields).toMatchObject({ org: "acme", project: "proj-9" });
  });

  it("uses GitHub's run as the build id and falls back to its ref for the branch", async () => {
    const dir = app();
    const result = await publish(dir, ["--dir", "out"], {
      OXY_TOKEN: "good-token",
      GITHUB_SHA: "abc123",
      GITHUB_RUN_ID: "42",
      GITHUB_RUN_ATTEMPT: "2",
      GITHUB_REF_NAME: "main"
    });
    expect(result.status, result.stderr).toBe(0);
    expect(uploads()[0]?.fields).toMatchObject({
      build_id: "abc123-42.2",
      commit_sha: "abc123",
      branch: "main"
    });
    // A commit with no repo is half a record — said out loud.
    expect(result.stderr).toContain("no git repo");
  });

  describe("trusted publishing", () => {
    const oidcEnv = () => ({
      ACTIONS_ID_TOKEN_REQUEST_URL: `${target}/github-oidc?api-version=2.0`,
      ACTIONS_ID_TOKEN_REQUEST_TOKEN: "gh-request-token"
    });

    const exchanges = () => received.filter((r) => r.path.includes("exchange")).map((r) => r.path);
    const MINTED = {
      status: 200,
      body: {
        token: "oxy_ci_minted",
        token_id: "tok-1",
        expires_at: "2099-01-01T00:00:00Z",
        service_account: "acme/deployer",
        grants: []
      }
    };

    /**
     * The older exchange, and the one every workflow written before trust
     * policies depends on. Such a workflow names no service account, so the
     * general exchange is NEVER ASKED — even on a deployment that has one,
     * and even if some policy there would match this run. The publish works
     * exactly as it did before trust policies existed.
     */
    it("with no service account named, goes straight to the app's registered publisher", async () => {
      // A general exchange that WOULD mint, to show it is not consulted.
      generalExchange = MINTED;
      const dir = app();
      const result = await publish(dir, ["--dir", "out"], oidcEnv());
      expect(result.status, result.stderr).toBe(0);
      const exchange = received.find((r) => r.path.endsWith("/oidc-exchange"));
      expect(JSON.parse(exchange?.exchangeBody ?? "{}")).toEqual({ app: "acme/sales" });
      expect(uploads()[0]?.authorization).toBe("Bearer minted");
      expect(exchanges()).toEqual(["/api/customer-apps/publish/oidc-exchange"]);
      expect(result.stderr).toContain("no service account is named");
      expect(result.stderr).toContain("OXY_SERVICE_ACCOUNT");
    });

    /** A deployment with no general exchange answers 404: the app's publisher is next. */
    it("with an account named, falls back to the app's publisher on a deployment with no general exchange", async () => {
      const dir = app();
      const result = await publish(dir, ["--dir", "out"], {
        ...oidcEnv(),
        OXY_SERVICE_ACCOUNT: SERVICE_ACCOUNT_ID
      });
      expect(result.status, result.stderr).toBe(0);
      expect(uploads()[0]?.authorization).toBe("Bearer minted");
      // Asked in order: the general exchange, then — on its 404 — the app's.
      expect(exchanges()).toEqual([
        "/api/auth/oidc/exchange",
        "/api/customer-apps/publish/oidc-exchange"
      ]);
      expect(result.stderr).toContain("no general token exchange");
    });

    it("acts as a service account when a trust policy matches, and revokes the token after", async () => {
      generalExchange = MINTED;
      const dir = app();
      const result = await publish(
        dir,
        ["--dir", "out", "--service-account", SERVICE_ACCOUNT_ID],
        oidcEnv()
      );
      expect(result.status, result.stderr).toBe(0);
      expect(uploads()[0]?.authorization).toBe("Bearer oxy_ci_minted");
      // The general exchange answered, so the app's publisher is never asked.
      expect(exchanges()).toEqual(["/api/auth/oidc/exchange"]);
      // The token was asked for this deployment's own host, worked out from the
      // target — the deployment was never asked which audience to use.
      expect(received.some((r) => r.path.startsWith("/api/auth/oidc/audience"))).toBe(false);
      const exchange = received.find((r) => r.path === "/api/auth/oidc/exchange");
      expect(JSON.parse(exchange?.exchangeBody ?? "{}")).toEqual({
        token: "gh-jwt-oxy",
        service_account: SERVICE_ACCOUNT_ID
      });
      // Minted by this process, so revoked by it — after the upload.
      const last = received.at(-1);
      expect(last).toMatchObject({
        method: "DELETE",
        path: "/api/auth/token",
        authorization: "Bearer oxy_ci_minted"
      });
    });

    it("falls back to the app's publisher when no trust policy matches", async () => {
      generalExchange = { status: 403, body: { error: "no policy", code: "no_matching_policy" } };
      const dir = app();
      const result = await publish(
        dir,
        ["--dir", "out", "--service-account", SERVICE_ACCOUNT_ID],
        oidcEnv()
      );
      expect(result.status, result.stderr).toBe(0);
      expect(uploads()[0]?.authorization).toBe("Bearer minted");
      expect(result.stderr).toContain(`no trust policy of ${SERVICE_ACCOUNT_ID} matches this run`);
    });

    it("says both were tried when neither a policy nor a publisher matches", async () => {
      generalExchange = { status: 403, body: { error: "no policy", code: "no_matching_policy" } };
      publisherRefuses = true;
      const dir = app();
      const result = await publish(
        dir,
        ["--dir", "out", "--service-account", SERVICE_ACCOUNT_ID],
        oidcEnv()
      );
      expect(result.status).toBe(ExitCode.AUTH);
      expect(result.stderr).toContain("not trusted to publish");
      // The fix it leads with is the current one — a trust policy.
      expect(result.stderr).toContain("oxyc init-ci");
      expect(uploads()).toHaveLength(0);
    });

    /**
     * With no publisher either, an unnamed run fails as it did before trust
     * policies existed: the publisher exchange's own refusal, nothing invented.
     */
    it("with no service account named and no publisher, reports the publisher's refusal", async () => {
      generalExchange = MINTED;
      publisherRefuses = true;
      const dir = app();
      const result = await publish(dir, ["--dir", "out"], oidcEnv());
      expect(result.status).toBe(ExitCode.AUTH);
      expect(result.stderr).toContain("trusted-publishing exchange failed");
      expect(exchanges()).toEqual(["/api/customer-apps/publish/oidc-exchange"]);
      expect(uploads()).toHaveLength(0);
    });

    /**
     * Every refusal but "no policy" names something wrong with THIS run.
     * Getting past it through the older exchange would hide that until the
     * older exchange is gone.
     */
    it("does not fall back on any other refusal", async () => {
      generalExchange = {
        status: 403,
        body: { error: "no environment", code: "missing_environment" }
      };
      const dir = app();
      const result = await publish(
        dir,
        ["--dir", "out", "--service-account", SERVICE_ACCOUNT_ID],
        oidcEnv()
      );
      expect(result.status).toBe(ExitCode.AUTH);
      expect(result.stderr).toContain("environment");
      expect(exchanges()).toEqual(["/api/auth/oidc/exchange"]);
      expect(uploads()).toHaveLength(0);
    });

    /**
     * `publish` never reads the exchange's `app_id` — only `checks run` does —
     * so a deployment predating that field must still publish. The field was
     * briefly required here, which would have failed the load-bearing path over
     * something it does not use.
     */
    it("publishes against a deployment whose exchange returns no app_id", async () => {
      noAppId = true;
      const dir = app();
      const result = await publish(dir, ["--dir", "out"], oidcEnv());
      expect(result.status, result.stderr).toBe(0);
      expect(uploads()[0]?.authorization).toBe("Bearer minted");
    });

    /** A set token is the operator's explicit choice; OIDC is only the fallback. */
    it("prefers a token that is set", async () => {
      const dir = app();
      const result = await publish(dir, ["--dir", "out"], {
        ...oidcEnv(),
        OXY_TOKEN: "good-token"
      });
      expect(result.status, result.stderr).toBe(0);
      expect(received.some((r) => r.path.endsWith("/oidc-exchange"))).toBe(false);
    });

    /** The exchange is registered by slug; a UUID cannot be matched. */
    it("needs the org slug", async () => {
      const dir = app({ slug: "sales" });
      const result = await publish(
        dir,
        ["--dir", "out", "--org", "0b0e5a10-1111-4222-8333-944455556666", "--project", "proj-1"],
        oidcEnv()
      );
      expect(result.status).toBe(ExitCode.USAGE);
      expect(result.stderr).toContain("org SLUG");
      expect(uploads()).toHaveLength(0);
    });
  });

  describe("split CI jobs", () => {
    it("--build-only builds and bundles without a credential or the network", async () => {
      const dir = app({
        slug: "sales",
        orgSlug: "acme",
        build: { install: "true", command: "mkdir -p out && echo built > out/index.html" }
      });
      const result = await publish(dir, ["--build-only", "--json"]);
      expect(result.status, result.stderr).toBe(0);
      // Resolved: the child's cwd is the real path, and a temp dir is often a symlink.
      expect(JSON.parse(result.stdout)).toEqual({ bundle_dir: join(realpathSync(dir), "out") });
      expect(received).toHaveLength(0);
    });

    /** The server does not check, so a missing function would 404 at runtime. */
    it("--prebuilt refuses a bundle missing a declared function", async () => {
      const dir = app({ slug: "sales", orgSlug: "acme", functions: { refresh: {} } });
      const result = await publish(dir, ["--dir", "out", "--prebuilt"], {
        OXY_TOKEN: "good-token"
      });
      expect(result.status).toBe(ExitCode.USAGE);
      expect(result.stderr).toContain("functions/refresh.js");
      expect(uploads()).toHaveLength(0);
    });

    it("--prebuilt publishes the functions it finds, without running esbuild", async () => {
      const dir = app({ slug: "sales", orgSlug: "acme", functions: { refresh: {} } });
      mkdirSync(join(dir, "out", "functions"));
      writeFileSync(join(dir, "out", "functions", "refresh.js"), "export default () => 1");
      const result = await publish(dir, ["--dir", "out", "--prebuilt"], {
        OXY_TOKEN: "good-token",
        // No pnpm on PATH: bundling would fail, so success proves it did not run.
        PATH: dirname(process.execPath)
      });
      expect(result.status, result.stderr).toBe(0);
      expect(tarList(uploads()[0]?.bundle ?? Buffer.alloc(0)).sort()).toEqual([
        "functions/",
        "functions/refresh.js",
        "index.html"
      ]);
    });

    it("--prebuilt needs --dir", async () => {
      const result = await publish(app(), ["--prebuilt"], { OXY_TOKEN: "good-token" });
      expect(result.status).toBe(ExitCode.USAGE);
    });
  });

  describe("the Oxy Functions lint", () => {
    /** An app with one function: its source in `functions/`, its bundle already in `out/`. */
    function appWithFunction(
      source: string,
      spec: Record<string, unknown> = {},
      manifest: Record<string, unknown> = {}
    ): string {
      const dir = app({
        slug: "sales",
        orgSlug: "acme",
        functions: { refresh: spec },
        ...manifest
      });
      // Recursive: a test that builds two apps reuses the one `app/` directory.
      mkdirSync(join(dir, "functions"), { recursive: true });
      writeFileSync(join(dir, "functions", "refresh.ts"), source);
      mkdirSync(join(dir, "out", "functions"), { recursive: true });
      writeFileSync(join(dir, "out", "functions", "refresh.js"), "export default () => 1");
      return dir;
    }
    const PREBUILT = ["--dir", "out", "--prebuilt"];
    // No pnpm on PATH: nothing here may need esbuild.
    const ENV = { OXY_TOKEN: "good-token", PATH: dirname(process.execPath) };
    const SEND =
      "export default async (r: unknown, ctx: any) => ctx.email.send({ to: 'a@b.c' });\n";
    const UPSERT =
      'export default async (r: unknown, ctx: any) => ctx.warehouse.upsert("ch", "t", [], ["k"]);\n';

    it("refuses a call whose capability the manifest lacks, before the build and the upload", async () => {
      const dir = app({
        slug: "sales",
        orgSlug: "acme",
        functions: { refresh: {} },
        build: { install: "touch installed", command: "true" }
      });
      mkdirSync(join(dir, "functions"));
      writeFileSync(join(dir, "functions", "refresh.ts"), SEND);
      const result = await publish(dir, [], ENV);
      expect(result.status).toBe(ExitCode.FAILURE);
      expect(result.stderr).toContain("1 Oxy Function lint problem(s)");
      expect(result.stderr).toContain(
        "[function-lint/capability] functions/refresh.ts (line 1): `ctx.email.send(` needs the `email.send` capability"
      );
      expect(result.stderr).toContain(
        'add "email": { "send": true } to functions.refresh in oxy-app.json'
      );
      expect(result.stderr).toContain("--allow-function-lint");
      expect(existsSync(join(dir, "installed"))).toBe(false);
      expect(uploads()).toHaveLength(0);
    });

    it("--allow-function-lint publishes anyway, each finding a warning that names its rule", async () => {
      const dir = appWithFunction(SEND);
      const result = await publish(dir, [...PREBUILT, "--allow-function-lint"], ENV);
      expect(result.status, result.stderr).toBe(0);
      expect(result.stderr).toContain(
        "warning: [function-lint/capability] functions/refresh.ts (line 1): `ctx.email.send(`"
      );
      expect(uploads()).toHaveLength(1);
    });

    it("asks the target which engine a write lands on, and refuses an upsert ClickHouse cannot parse", async () => {
      const dir = appWithFunction(UPSERT, {
        destinations: ["ch"],
        customerWarehouseWrites: { ch: "legacy rollups" }
      });
      const result = await publish(dir, PREBUILT, ENV);
      expect(result.status).toBe(ExitCode.FAILURE);
      expect(result.stderr).toContain(
        '[function-lint/engine] functions/refresh.ts (line 1): `ctx.warehouse.upsert("ch", …)` — `ch` is clickhouse'
      );
      expect(
        received.some(
          (r) => r.path === "/api/proj-1/databases" && r.authorization === "Bearer good-token"
        )
      ).toBe(true);
      expect(uploads()).toHaveLength(0);
    });

    it("refuses a customer-warehouse write with no reason, and takes one into Airhouse", async () => {
      const noReason = await publish(
        appWithFunction(UPSERT, { destinations: ["ch"] }),
        PREBUILT,
        ENV
      );
      expect(noReason.status).toBe(ExitCode.FAILURE);
      expect(noReason.stderr).toContain("[function-lint/customer-warehouse]");
      expect(noReason.stderr).toContain("[function-lint/engine]");

      const airhouse = appWithFunction(
        'export default async (r: unknown, ctx: any) => ctx.warehouse.insert("ah", "t", []);\n',
        { destinations: ["ah"] }
      );
      const result = await publish(airhouse, PREBUILT, ENV);
      expect(result.status, result.stderr).toBe(0);
      expect(result.stderr).not.toContain("function-lint");
      expect(uploads()).toHaveLength(1);
    });

    it("skips the engine check with one warning when the target will not list the databases", async () => {
      databasesStatus = 403;
      const dir = appWithFunction(UPSERT, { destinations: ["ch"] });
      const result = await publish(dir, PREBUILT, ENV);
      expect(result.status, result.stderr).toBe(0);
      expect(result.stderr).toContain("function lint: the engine check");
      expect(result.stderr).toContain("was skipped");
      expect(result.stderr).toContain("answered 403");
      expect(uploads()).toHaveLength(1);
    });
  });
});

describe("inferOrgApp", () => {
  it("reads apps/<org>/<app> from the working directory", () => {
    expect(inferOrgApp("/r/apps/acme/sales")).toEqual({ org: "acme", app: "sales" });
    expect(inferOrgApp("/r/apps/acme/sales/src")).toEqual({ org: "acme", app: "sales" });
    expect(inferOrgApp("/r/elsewhere")).toEqual({});
  });
});
