#!/usr/bin/env node
/**
 * Checkpoint 1 of the custom-app guard: publish THIS checkout's platform canary
 * to a running server and run its checks against real engines.
 *
 * The staging and prod canaries (`custom-app-checks.yaml`) run after a release
 * is cut, so a break in the publish path, the SDK build, the manifest or a host
 * API is found at release time. This runs the same canary on a pull request,
 * against the PR's own server, SDK, Vite plugin and `oxyc` — so the break is
 * found on the PR. CI runs it from `ci.yaml` (`custom-app-canary`); an engineer
 * runs it against a dev box with `just custom-app-canary`.
 *
 * Given a base URL it:
 *
 *   1. builds `@oxy-hq/sdk`, `@oxy-hq/vite-plugin` and `@oxy-hq/cli` from the
 *      workspace, and `pnpm pack`s the first two;
 *   2. stages a copy of `customer-apps/examples/platform-canary` whose
 *      `pnpm-workspace.yaml` overrides both packages to those tarballs — the
 *      committed canary keeps depending on npm, so a laptop `pnpm install` there
 *      is unchanged, while this build compiles against the checkout's SDK;
 *   3. creates or reuses the canary's org, user, grant, ClickHouse database,
 *      OLTP writer and workspace database, then compiles and promotes;
 *   4. publishes with the workspace `oxyc`, sets `CANARY_STEPS` to every step
 *      the canary declares minus `OMITTED_IN_CI` below, and runs
 *      `oxyc checks run` twice: the first run may fail only `secrets_roundtrip`
 *      (it reads the value the previous run wrote, so a fresh stack has none),
 *      the second must be green and must have run exactly the configured
 *      steps (read back from the run's answer). Any other failure exits 1
 *      naming the step;
 *   5. with `--journey`, opens the published app in Chromium the way the
 *      release checks do (`web-app/tests/custom-app-checks/app-loads.spec.ts`,
 *      X-API-Key, the ready beacon, `[data-canary="ready"]`). That is the
 *      API-key journey of checkpoints 2 and 3, not the design's real sign-in
 *      on a custom-app subdomain, which needs a cookie domain, a host-resolver
 *      rule in Chromium and a login page this server may not embed.
 *
 * Every identity comes from a supported path: `dev-login` mints the users, the
 * admin API creates the org, membership, grant and flag, the workspace API adds
 * the database and mints the key. Nothing writes to Postgres by hand.
 *
 * The server must have dev sign-in on for two addresses — a staff one in
 * `OXY_GLOBAL_ADMINS` and the canary user in `OXY_DEV_LOGIN_EMAILS` — plus
 * `OXY_OLTP_PROVIDER=local` with an admin URL, so `ctx.oltp` has a tenant. See
 * the `custom-app-canary` job in `.github/workflows/ci.yaml` for the full
 * environment, and the canary README, "Running it in CI and locally".
 */

import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const CANARY_DIR = join(REPO, "customer-apps", "examples", "platform-canary");
const OXYC = join(REPO, "sdk", "cli", "dist", "main.mjs");
const APP_SLUG = "platform-canary";
/** The app's OLTP writer: the slug with hyphens as underscores, as the platform derives it. */
const OLTP_WRITER = "app:platform_canary";
const WAREHOUSE_NAME = "canary_warehouse";
const STAFF_EMAIL = "canary-staff@oxy.test";
const CANARY_EMAIL = "canary@oxygen-hq.com";

/** Where the canary declares its steps. `ALL_STEPS` there is the only list of them. */
export const ALL_STEPS_SOURCE = join(CANARY_DIR, "functions", "steps.ts");

/**
 * The steps CI does NOT run, each with its reason. The CI list is DERIVED —
 * `ALL_STEPS` from the canary's own steps.ts minus this — so a step added to
 * the canary runs here the day it lands, and the only way to keep one out is
 * to name it here and say why (the recorded decision `CANARY_STEPS` is meant
 * to be; canary README, "Configuration"). `platform-canary-checkpoint.test.mjs`
 * fails when a name here is not a real step. Never fake a step.
 *
 * `oltp_roundtrip`, `oltp_transaction` and the Postgres half of `shape_zoo`
 * run: `oxy serve` with `OXY_OLTP_PROVIDER=local` provisions each org as a
 * database in the cluster its admin URL names. `upsert_refusal` and
 * `tx_refusal` pin engine refusals the server on the same tree classes as
 * `bad_request`; the job builds the tree it tests, so they hold here.
 */
export const OMITTED_IN_CI = {
  check_in:
    "it POSTs to an All Quiet cron check-in URL and fails closed without one; " +
    "CI has no monitor to check in to",
  storage_roundtrip:
    "its presigned PUT goes through ctx.fetch, whose egress policy allows only HTTPS " +
    "to a public host (host.rs is_safe_outbound); CI's object store would be MinIO on " +
    "loopback, so the PUT is refused before it leaves the isolate. Runnable once the " +
    "runner has a public HTTPS bucket, or the policy takes a test-only loopback opt-in"
};

/**
 * `ALL_STEPS` out of the canary's steps.ts, read as text the way the manifest
 * is read as JSON — never a second hand-written copy. Exact or nothing: the
 * array literal must hold only quoted names (and comments), or this throws
 * rather than silently dropping an entry it could not read.
 */
export function parseAllSteps(source) {
  const m = /export const ALL_STEPS\b[^=]*=\s*\[([\s\S]*?)\];/.exec(source);
  if (!m) throw new Error(`no \`export const ALL_STEPS = [...]\` in ${ALL_STEPS_SOURCE}`);
  const body = m[1].replace(/\/\/[^\n]*/g, "");
  const names = [...body.matchAll(/"([a-z][a-z0-9_]*)"/g)].map((x) => x[1]);
  const leftover = body.replace(/"[a-z][a-z0-9_]*"/g, "").replace(/[\s,]/g, "");
  if (names.length === 0 || leftover !== "") {
    throw new Error(
      `ALL_STEPS in ${ALL_STEPS_SOURCE} holds something other than quoted step names`
    );
  }
  if (new Set(names).size !== names.length) throw new Error("ALL_STEPS repeats a name");
  return names;
}

/** `all` minus the omissions, in the canary's order; an omission of a non-step throws. */
export function deriveCiSteps(all, omitted) {
  for (const name of Object.keys(omitted)) {
    if (!all.includes(name))
      throw new Error(`OMITTED_IN_CI names ${name}, which is not in ALL_STEPS`);
  }
  return all.filter((step) => !(step in omitted));
}

/** The one first-run failure a fresh stack produces by design. */
const PRIMING_STEP = "secrets_roundtrip";

const USAGE = `usage: node scripts/ci/platform-canary-checkpoint.mjs --target <url> [options]

  --target <url>                 the running server (env CANARY_TARGET)
  --clickhouse-url <url>         ClickHouse HTTP endpoint the SERVER can reach
                                 (env CANARY_CLICKHOUSE_URL, default http://localhost:8123)
  --clickhouse-user <user>       (env CANARY_CLICKHOUSE_USER, default "default")
  --clickhouse-password <pw>     (env CANARY_CLICKHOUSE_PASSWORD, default "")
  --clickhouse-database <name>   (env CANARY_CLICKHOUSE_DATABASE, default oxy_canary)
  --no-create-database           do not CREATE DATABASE on ClickHouse first
  --org-slug <slug>              the canary's org (default oxy-canary)
  --steps <csv|all>              CANARY_STEPS to set (default: ALL_STEPS minus OMITTED_IN_CI)
  --list-steps                   print the derived list and the omissions, then exit
  --no-build                     reuse sdk/*/dist as built
  --keep                         leave the staged app and tarballs on disk
  --journey                      also run the browser check (needs Playwright's Chromium in web-app)
  --check-timeout <seconds>      per-check timeout passed to oxyc (default 240)
`;

/** An environment variable, trimmed; blank counts as absent. */
function env(name, fallback) {
  return process.env[name]?.trim() || fallback;
}

function parseArgs(argv) {
  const opts = {
    target: env("CANARY_TARGET"),
    clickhouseUrl: env("CANARY_CLICKHOUSE_URL", "http://localhost:8123"),
    clickhouseUser: env("CANARY_CLICKHOUSE_USER", "default"),
    clickhousePassword: env("CANARY_CLICKHOUSE_PASSWORD", ""),
    clickhouseDatabase: env("CANARY_CLICKHOUSE_DATABASE", "oxy_canary"),
    createDatabase: true,
    orgSlug: "oxy-canary",
    steps: undefined,
    listSteps: false,
    build: true,
    keep: false,
    journey: false,
    checkTimeout: 240
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      const v = argv[++i];
      if (v === undefined) fail(`${a} needs a value\n${USAGE}`);
      return v;
    };
    if (a === "--target") opts.target = next();
    else if (a === "--clickhouse-url") opts.clickhouseUrl = next();
    else if (a === "--clickhouse-user") opts.clickhouseUser = next();
    else if (a === "--clickhouse-password") opts.clickhousePassword = next();
    else if (a === "--clickhouse-database") opts.clickhouseDatabase = next();
    else if (a === "--no-create-database") opts.createDatabase = false;
    else if (a === "--org-slug") opts.orgSlug = next();
    else if (a === "--steps") opts.steps = next();
    else if (a === "--list-steps") opts.listSteps = true;
    else if (a === "--no-build") opts.build = false;
    else if (a === "--keep") opts.keep = true;
    else if (a === "--journey") opts.journey = true;
    else if (a === "--check-timeout") opts.checkTimeout = Number(next());
    else if (a === "-h" || a === "--help") {
      process.stdout.write(USAGE);
      process.exit(0);
    } else fail(`unknown argument ${a}\n${USAGE}`);
  }
  if (!opts.listSteps && !opts.target) fail(`--target (or CANARY_TARGET) is required\n${USAGE}`);
  if (opts.target) opts.target = opts.target.replace(/\/+$/, "");
  if (!Number.isFinite(opts.checkTimeout) || opts.checkTimeout <= 0) {
    fail("--check-timeout must be a positive number of seconds");
  }
  return opts;
}

// ── output ───────────────────────────────────────────────────────────────────

const inActions = Boolean(env("GITHUB_ACTIONS"));

function step(title) {
  process.stdout.write(`\n== ${title}\n`);
}

function note(line) {
  process.stdout.write(`   ${line}\n`);
}

/** Exit 1 with one line that names what failed. The same line becomes a GitHub annotation. */
function fail(message) {
  if (inActions) process.stdout.write(`::error::${message.split("\n")[0]}\n`);
  process.stderr.write(`\nFAIL: ${message}\n`);
  process.exit(1);
}

// ── processes ────────────────────────────────────────────────────────────────

/** Run a command to completion, output inherited; a non-zero exit is a failure named by `what`. */
function run(what, cmd, args, { cwd = REPO, env = {} } = {}) {
  note(`$ ${[cmd, ...args].join(" ")}`);
  const r = spawnSync(cmd, args, { cwd, stdio: "inherit", env: { ...process.env, ...env } });
  if (r.error) fail(`${what}: could not start ${cmd}: ${r.error.message}`);
  if (r.status !== 0)
    fail(`${what}: \`${cmd} ${args.join(" ")}\` exited ${r.status ?? "by signal"}`);
}

/** Run a command and capture stdout (stderr inherited); returns `{ status, stdout }`. */
function capture(cmd, args, { cwd = REPO, env = {} } = {}) {
  note(`$ ${[cmd, ...args].join(" ")}`);
  const r = spawnSync(cmd, args, {
    cwd,
    stdio: ["ignore", "pipe", "inherit"],
    env: { ...process.env, ...env },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024
  });
  if (r.error) fail(`could not start ${cmd}: ${r.error.message}`);
  return { status: r.status, stdout: r.stdout ?? "" };
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

/** JSON in, JSON (or text) out. Never throws on a status; the caller decides. */
async function api(target, token, method, path, body) {
  const headers = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  let response;
  try {
    response = await fetch(`${target}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(60_000)
    });
  } catch (cause) {
    fail(`${method} ${path}: ${cause.message}`);
  }
  const text = await response.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = undefined;
  }
  return { status: response.status, json, text };
}

/** `expect` names the call; anything outside 2xx fails with the status and a short body. */
async function must(target, token, method, path, body, expect) {
  const r = await api(target, token, method, path, body);
  if (r.status < 200 || r.status >= 300) {
    fail(`${expect}: ${method} ${path} answered ${r.status} ${r.text.slice(0, 300)}`);
  }
  return r;
}

async function devLogin(target, query, who) {
  const r = await api(target, undefined, "GET", `/api/auth/dev-login?${query}`);
  if (r.status === 404) {
    fail(
      `dev sign-in is off on ${target} (404): the server needs OXY_GLOBAL_ADMINS and ` +
        `OXY_DEV_LOGIN_EMAILS=${STAFF_EMAIL},${CANARY_EMAIL}`
    );
  }
  if (r.status !== 200 || !r.json?.token) {
    fail(`dev-login as ${who}: ${r.status} ${r.text.slice(0, 200)}`);
  }
  return r.json;
}

async function sleep(ms) {
  await new Promise((r) => setTimeout(r, ms));
}

// ── 1. build and pack the workspace packages ─────────────────────────────────

function buildAndPack(opts, work) {
  step("workspace SDK, Vite plugin and oxyc");
  if (opts.build) {
    for (const pkg of ["@oxy-hq/sdk", "@oxy-hq/vite-plugin", "@oxy-hq/cli"]) {
      run(`build ${pkg}`, "pnpm", ["--filter", pkg, "build"]);
    }
  } else {
    note("--no-build: using sdk/*/dist as they are");
  }
  if (!existsSync(OXYC)) fail(`no oxyc build at ${OXYC}; drop --no-build`);
  const tarballs = {};
  for (const pkg of ["@oxy-hq/sdk", "@oxy-hq/vite-plugin"]) {
    const r = capture("pnpm", [
      "--filter",
      pkg,
      "pack",
      "--json",
      "--pack-destination",
      join(work, "pack")
    ]);
    let filename;
    try {
      filename = JSON.parse(r.stdout).filename;
    } catch {
      filename = undefined;
    }
    if (r.status !== 0 || !filename || !existsSync(filename))
      fail(`pack ${pkg}: exited ${r.status}, no tarball`);
    tarballs[pkg] = filename;
    note(`${pkg} → ${filename.split("/").pop()}`);
  }
  return tarballs;
}

// ── 2. stage the canary against those tarballs ───────────────────────────────

function stageCanary(work, tarballs) {
  step("stage the canary against the workspace packages");
  const app = join(work, "app");
  cpSync(CANARY_DIR, app, {
    recursive: true,
    filter: (src) => !/(^|\/)(node_modules|out|dist)(\/|$)/.test(src.slice(CANARY_DIR.length))
  });
  // pnpm 10+ reads overrides from pnpm-workspace.yaml; the canary already has
  // one (its esbuild build decision), so append to it. `file:` tarballs, not
  // `link:`, so the copy resolves one React and the package's `files` list is
  // what gets installed — the same bytes `npm publish` would ship.
  const overrides = Object.entries(tarballs)
    .map(([pkg, path]) => `  "${pkg}": "file:${path}"`)
    .join("\n");
  // The committed lockfile pins the npm packages; overriding them means the
  // lockfile must be updated, which a CI install (frozen by default) refuses
  // with ERR_PNPM_LOCKFILE_CONFIG_MISMATCH. `frozenLockfile: false` here is the
  // one form pnpm 11 honours under CI: an `.npmrc` `frozen-lockfile=false` and
  // `npm_config_frozen_lockfile=false` are both ignored (tested), and this
  // keeps the manifest's own `pnpm install` as the install step.
  writeFileSync(
    join(app, "pnpm-workspace.yaml"),
    `\noverrides:\n${overrides}\nfrozenLockfile: false\n`,
    { flag: "a" }
  );
  note(`staged at ${app}`);
  return app;
}

// ── 3. identities, org, engines, workspace ───────────────────────────────────

async function ensureOrg(opts, staff, canary) {
  step(`org ${opts.orgSlug}`);
  // Owner, not member: minting the workspace API key needs WorkspaceAdmin, which
  // an org Owner derives. Prod's canary is a Member holding only the operator
  // grant below, whose key a human minted once; this CI-only org does not model
  // that, and the grant is what publish and `checks run` need either way.
  const created = await api(opts.target, staff.token, "POST", "/api/admin/orgs", {
    name: `Oxy Canary (${opts.orgSlug})`,
    slug: opts.orgSlug,
    owner_email: CANARY_EMAIL
  });
  let orgId;
  let workspaceId;
  if (created.status === 200) {
    orgId = created.json.org.id;
    workspaceId = created.json.default_workspace_id;
    note(`created org ${orgId} with owner ${CANARY_EMAIL}`);
  } else if (created.status === 409) {
    const again = await devLogin(
      opts.target,
      `email=${encodeURIComponent(CANARY_EMAIL)}`,
      "canary"
    );
    const org = (again.orgs ?? []).find((o) => o.slug === opts.orgSlug);
    if (!org) {
      fail(
        `org ${opts.orgSlug} exists but ${CANARY_EMAIL} is not a member of it; use another --org-slug`
      );
    }
    orgId = org.id;
    const list = await must(
      opts.target,
      canary.token,
      "GET",
      `/api/orgs/${orgId}/workspaces`,
      undefined,
      "list workspaces"
    );
    const ws = list.json.find((w) => w.name === "Default") ?? list.json[0];
    if (!ws) fail(`org ${opts.orgSlug} has no workspace`);
    workspaceId = ws.id;
    note(`reusing org ${orgId} (${org.role}), workspace ${workspaceId}`);
  } else {
    fail(
      `create org: POST /api/admin/orgs answered ${created.status} ${created.text.slice(0, 300)}`
    );
  }

  // The operator grant, scoped to this org: what `oxyc publish` and
  // `oxyc checks run` (both on /api/admin/**) need. Prod's canary has the same.
  const grant = await api(opts.target, staff.token, "POST", "/api/admin/app-admins", {
    email: CANARY_EMAIL,
    role: "app_operator",
    scope_org_ids: [orgId]
  });
  if (grant.status === 409) note("operator grant already exists");
  else if (grant.status < 200 || grant.status >= 300) {
    fail(`grant app_operator: ${grant.status} ${grant.text.slice(0, 300)}`);
  } else note(`granted app_operator scoped to ${opts.orgSlug}`);

  return { orgId, workspaceId };
}

async function ensureOltp(opts, staff, orgId) {
  step("per-org OLTP");
  // The global kill switch: off by default, and provisioning fails closed.
  await must(
    opts.target,
    staff.token,
    "PATCH",
    "/api/admin/feature-flags/oltp",
    { enabled: true },
    "enable oltp flag"
  );
  // Every instance's flag cache refreshes within ~15s; provisioning reads it, so
  // a refusal right after the flip is the cache, not the provider. Retry briefly.
  const deadline = Date.now() + 45_000;
  for (;;) {
    const r = await api(
      opts.target,
      staff.token,
      "POST",
      `/api/admin/orgs/${orgId}/oltp/provision`,
      {
        writers: [OLTP_WRITER]
      }
    );
    if (r.status >= 200 && r.status < 300) {
      note(
        `provisioned ${r.json?.database ?? "?"} on ${r.json?.host ?? "?"} (${r.json?.provider ?? "?"}), writer ${OLTP_WRITER}`
      );
      return;
    }
    if (Date.now() >= deadline) {
      fail(`provision OLTP: ${r.status} ${r.text.slice(0, 300)}`);
    }
    note(`provision answered ${r.status}; retrying`);
    await sleep(3000);
  }
}

async function ensureClickHouse(opts) {
  step(`ClickHouse database ${opts.clickhouseDatabase} on ${opts.clickhouseUrl}`);
  if (!opts.createDatabase) {
    note("--no-create-database: not creating it");
    return;
  }
  const url = `${opts.clickhouseUrl.replace(/\/+$/, "")}/?query=${encodeURIComponent(
    `CREATE DATABASE IF NOT EXISTS ${opts.clickhouseDatabase}`
  )}`;
  let r;
  try {
    r = await fetch(url, {
      method: "POST",
      headers: {
        "X-ClickHouse-User": opts.clickhouseUser,
        "X-ClickHouse-Key": opts.clickhousePassword
      },
      signal: AbortSignal.timeout(30_000)
    });
  } catch (cause) {
    fail(`ClickHouse at ${opts.clickhouseUrl}: ${cause.message}`);
  }
  if (!r.ok) fail(`CREATE DATABASE on ClickHouse: ${r.status} ${(await r.text()).slice(0, 200)}`);
  note("exists");
}

async function ensureWarehouse(opts, canary, workspaceId) {
  step(`workspace database ${WAREHOUSE_NAME}`);
  const list = await must(
    opts.target,
    canary.token,
    "GET",
    `/api/${workspaceId}/databases`,
    undefined,
    "list databases"
  );
  if (list.json.some((d) => d.name === WAREHOUSE_NAME)) {
    note("already configured; leaving it (reconfigure by using a fresh --org-slug)");
    return;
  }
  // The password lands in the workspace secrets store as `<NAME>_PASSWORD`, and
  // config.yml carries only its `password_var`. The one database is also the
  // default, which is what `sql_read`'s `ctx.query` reaches.
  await must(
    opts.target,
    canary.token,
    "POST",
    `/api/${workspaceId}/databases`,
    {
      warehouses: [
        {
          type: "clickhouse",
          name: WAREHOUSE_NAME,
          config: {
            host: opts.clickhouseUrl,
            user: opts.clickhouseUser,
            password: opts.clickhousePassword,
            database: opts.clickhouseDatabase
          }
        }
      ]
    },
    "add database"
  );
  note(`added ${WAREHOUSE_NAME} → ${opts.clickhouseDatabase}`);
}

async function compileAndPromote(opts, staff, canary, workspaceId) {
  step("compile and promote the workspace");
  // Not `POST /{ws}/compile`: that route ships the default branch's HEAD, and a
  // blank org workspace has no commit, so it answers 409. The admin route is what
  // the console's "Run compile now" uses and takes a workspace without a SHA.
  await must(
    opts.target,
    staff.token,
    "POST",
    "/api/admin/compiles/run",
    { workspace_id: workspaceId, promote: true },
    "enqueue compile"
  );
  const deadline = Date.now() + 180_000;
  for (;;) {
    const s = await must(
      opts.target,
      canary.token,
      "GET",
      `/api/${workspaceId}/compile/status?branch=main`,
      undefined,
      "compile status"
    );
    const latest = s.json.latest;
    if (latest?.status === "ready" && s.json.current_revision_id === latest.revision_id) {
      note(`revision ${latest.revision_id} ready and promoted (${latest.duration_ms ?? "?"} ms)`);
      return;
    }
    if (latest?.status === "failed") fail(`compile failed: revision ${latest.revision_id}`);
    if (Date.now() >= deadline)
      fail(`compile did not promote within 180s (latest: ${JSON.stringify(latest)})`);
    await sleep(1000);
  }
}

async function mintApiKey(opts, canary, workspaceId) {
  step("API key for the canary user");
  const expires = new Date(Date.now() + 24 * 3600 * 1000).toISOString();
  const r = await must(
    opts.target,
    canary.token,
    "POST",
    `/api/${workspaceId}/api-keys`,
    { name: `platform-canary-checkpoint ${new Date().toISOString()}`, expires_at: expires },
    "mint API key"
  );
  note(`minted ${r.json.masked_key}, expires ${expires}`);
  return r.json.key;
}

// ── 4. publish, configure, check ─────────────────────────────────────────────

function publish(opts, app, canary, workspaceId, work) {
  step("oxyc publish (workspace build of oxyc)");
  // A dev-login JWT is an ordinary session bearer, and publish accepts it — the
  // verification design (§9) listed that as an open assumption; it holds. The
  // credentials path points at nothing so a laptop's real `oxyc login` cannot
  // leak into this run; `--project` because a first publish has no build-config
  // to look the workspace up from.
  const r = capture(
    "node",
    [
      OXYC,
      "publish",
      "--target",
      opts.target,
      "--promote",
      "--project",
      workspaceId,
      "--org",
      opts.orgSlug,
      "--token-env",
      "CANARY_JWT",
      "--json"
    ],
    {
      cwd: app,
      env: { CANARY_JWT: canary.token, OXY_CREDENTIALS_PATH: join(work, "no-credentials.json") }
    }
  );
  if (r.status !== 0) fail(`oxyc publish exited ${r.status} (see the log above)`);
  let result;
  try {
    result = JSON.parse(r.stdout);
  } catch {
    fail(`oxyc publish printed no JSON result: ${r.stdout.slice(0, 200)}`);
  }
  note(
    `${result.is_new_app ? "registered" : "published"} ${opts.orgSlug}/${APP_SLUG} build ${result.build_id} → ${result.channel}`
  );
  return result.app_id;
}

async function configureSteps(opts, canary, appId) {
  step("CANARY_STEPS");
  const path = `/api/customer-apps/${appId}/secrets`;
  if (opts.steps === "all") {
    const r = await api(opts.target, canary.token, "DELETE", `${path}/CANARY_STEPS`);
    if (r.status !== 404 && (r.status < 200 || r.status >= 300)) {
      fail(`clear CANARY_STEPS: ${r.status} ${r.text.slice(0, 200)}`);
    }
    note("unset: every step runs, check_in included");
    return;
  }
  await must(
    opts.target,
    canary.token,
    "POST",
    path,
    { key: "CANARY_STEPS", value: opts.steps },
    "set CANARY_STEPS"
  );
  note(opts.steps);
}

/** The step a failed check names, from the canary's `canary step <name> failed: …`. */
function failedStep(error) {
  return /canary step ([a-z_]+) failed/.exec(error ?? "")?.[1];
}

/** What a non-9 `oxyc checks run` exit means (its exit-code contract). */
function whyNotChecked(code) {
  switch (code) {
    case 1:
      return "the app declares no checks";
    case 4:
      return "no credential resolved, or the API rejected the key (401/403)";
    case 5:
      return "no such app on the target";
    case 6:
      return "the API refused the request (4xx)";
    case 7:
      return "the API answered 5xx or was unreachable";
    default:
      return "see `oxyc exit-codes`";
  }
}

async function runChecks(opts, apiKey, canary, work, label) {
  step(`oxyc checks run (${label})`);
  // The API key, the way the release checks authenticate: `--token-env` names a
  // variable that is never set and the credentials path is empty, so no bearer
  // resolves and oxyc sends X-API-Key as the canary user.
  const r = capture(
    "node",
    [
      OXYC,
      "checks",
      "run",
      `${opts.orgSlug}/${APP_SLUG}`,
      "--target",
      opts.target,
      "--api-key-env",
      "CANARY_API_KEY",
      "--token-env",
      "CANARY_NO_BEARER",
      "--json",
      "--timeout",
      String(opts.checkTimeout)
    ],
    { env: { CANARY_API_KEY: apiKey, OXY_CREDENTIALS_PATH: join(work, "no-credentials.json") } }
  );
  let report;
  try {
    report = JSON.parse(r.stdout);
  } catch {
    report = undefined;
  }
  if (r.status !== 0 && r.status !== 9) {
    fail(`oxyc checks run exited ${r.status}: ${whyNotChecked(r.status)}; no check ran`);
  }
  if (!report?.checks) fail(`oxyc checks run exited ${r.status} but printed no report`);
  const ran = {};
  for (const c of report.checks) {
    const verdict = c.passed ? "passed" : `FAILED at step ${failedStep(c.error) ?? "(unnamed)"}`;
    note(
      `${c.name}: ${verdict} in ${(c.durationMs / 1000).toFixed(1)}s${c.passed ? "" : ` — ${c.error}`}`
    );
    if (c.passed) ran[c.name] = await stepsRan(opts, canary, report.appId, c.runId);
  }
  return { code: r.status, checks: report.checks, ran };
}

/**
 * The steps a passed run actually executed, from the canary's own answer
 * (`runCanary` returns `{ ok, runId, steps }`, and `canary.ts` answers it as
 * JSON). Read back so the log shows the names, and so a run that silently ran
 * fewer steps than configured cannot pass.
 */
async function stepsRan(opts, canary, appId, runId) {
  const r = await must(
    opts.target,
    canary.token,
    "GET",
    `/api/admin/apps/${appId}/function-runs/${runId}`,
    undefined,
    "read run detail"
  );
  let answer;
  try {
    answer = JSON.parse(r.json?.answer ?? "");
  } catch {
    fail(`run ${runId}: its answer is not the canary's JSON result`);
  }
  if (!Array.isArray(answer.steps)) fail(`run ${runId}: the answer names no steps`);
  note(`steps ran (${answer.steps.length}): ${answer.steps.join(", ")}`);
  return answer.steps;
}

/**
 * The browser check the release checks run against staging and prod, pointed
 * at this server. The key rides on requests to the app's own origin only (the
 * spec routes it), the config keeps screenshots and traces off, and nothing
 * here uploads the HTML report.
 */
function runJourney(opts, apiKey) {
  step("browser check (app-loads.spec.ts)");
  const url = `${opts.target}/customer-apps/${opts.orgSlug}/${APP_SLUG}/`;
  const targets = [
    { app: `${opts.orgSlug}/${APP_SLUG}`, url, expectSelector: '[data-canary="ready"]' }
  ];
  run(
    "browser check",
    "pnpm",
    [
      "exec",
      "playwright",
      "test",
      "-c",
      "playwright.custom-app-checks.config.ts",
      "--reporter=list"
    ],
    {
      cwd: join(REPO, "web-app"),
      env: {
        CUSTOM_APP_CHECK_TARGETS: JSON.stringify(targets),
        OXY_API_KEY: apiKey,
        CUSTOM_APP_CHECK_ALLOWED_ORIGINS: opts.target
      }
    }
  );
  note(`${url} mounted and rendered data-canary="ready"`);
}

// ── main ─────────────────────────────────────────────────────────────────────

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (!existsSync(join(CANARY_DIR, "oxy-app.json"))) fail(`no canary at ${CANARY_DIR}`);
  const allSteps = parseAllSteps(readFileSync(ALL_STEPS_SOURCE, "utf8"));
  const ciSteps = deriveCiSteps(allSteps, OMITTED_IN_CI);
  if (opts.listSteps) {
    process.stdout.write(
      `ALL_STEPS (${allSteps.length}, from ${ALL_STEPS_SOURCE}):\n  ${allSteps.join(", ")}\n`
    );
    process.stdout.write(`omitted in CI (${Object.keys(OMITTED_IN_CI).length}):\n`);
    for (const [name, reason] of Object.entries(OMITTED_IN_CI))
      process.stdout.write(`  ${name}: ${reason}\n`);
    process.stdout.write(`CI runs (${ciSteps.length}):\n  ${ciSteps.join(", ")}\n`);
    return;
  }
  opts.steps ??= ciSteps.join(",");
  const expected = opts.steps === "all" ? allSteps : opts.steps.split(",").map((x) => x.trim());
  const work = mkdtempSync(join(tmpdir(), "platform-canary-checkpoint-"));
  process.stdout.write(`platform canary checkpoint → ${opts.target}\n   work dir ${work}\n`);
  let ok = false;
  try {
    const tarballs = buildAndPack(opts, work);
    const app = stageCanary(work, tarballs);

    step("identities (dev-login)");
    const staff = await devLogin(opts.target, "as=staff", "staff");
    const canary = await devLogin(
      opts.target,
      `email=${encodeURIComponent(CANARY_EMAIL)}`,
      "canary"
    );
    note(`staff ${staff.user.email}; canary ${canary.user.email}`);

    const { orgId, workspaceId } = await ensureOrg(opts, staff, canary);
    await ensureOltp(opts, staff, orgId);
    await ensureClickHouse(opts);
    await ensureWarehouse(opts, canary, workspaceId);
    await compileAndPromote(opts, staff, canary, workspaceId);
    const apiKey = await mintApiKey(opts, canary, workspaceId);

    const appId = publish(opts, app, canary, workspaceId, work);
    await configureSteps(opts, canary, appId);

    // Run 1 primes `secrets_roundtrip`: the step reads the value the previous
    // run wrote, so on a fresh stack it fails once, and only it may. Every step
    // before it in the canary's order has passed by then, because the canary
    // stops at the first failure.
    const first = await runChecks(opts, apiKey, canary, work, "run 1, primes secrets_roundtrip");
    for (const c of first.checks) {
      if (c.passed) continue;
      const s = failedStep(c.error);
      if (s === PRIMING_STEP) {
        note(`tolerated: ${PRIMING_STEP} has nothing to read on the first run`);
        continue;
      }
      fail(`run 1: check ${c.name} failed at step ${s ?? "(unnamed)"}: ${c.error}`);
    }

    const second = await runChecks(opts, apiKey, canary, work, "run 2, must be green");
    for (const c of second.checks) {
      if (!c.passed) {
        fail(
          `run 2: check ${c.name} failed at step ${failedStep(c.error) ?? "(unnamed)"}: ${c.error}`
        );
      }
    }
    if (second.code !== 0) fail(`run 2: oxyc checks run exited ${second.code}`);
    // Exactly the configured steps, no more and no fewer: the canary runs what
    // `CANARY_STEPS` names, and its answer is the record of what that was.
    const ran = [...(second.ran.canary ?? [])].sort().join(",");
    const want = [...expected].sort().join(",");
    if (ran !== want) fail(`run 2 ran [${ran}] but CANARY_STEPS asked for [${want}]`);

    if (opts.journey) runJourney(opts, apiKey);

    step("green");
    note(
      `${opts.orgSlug}/${APP_SLUG} published from this checkout and every configured step passed twice` +
        (opts.journey ? ", and the app mounts in a browser" : "")
    );
    ok = true;
  } finally {
    if (opts.keep || !ok) note(`work dir kept at ${work}`);
    else rmSync(work, { recursive: true, force: true });
  }
}

// Importable by its test without running: `main` runs only when this file is
// the entry point.
const entry = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (import.meta.url === entry) await main();
