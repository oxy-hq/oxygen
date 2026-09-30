import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, openSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { flowEmail } from "./session";
import type { BackendMode } from "./types";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
// The fixture every flow is authored against: the runner seeds it as the
// enterprise Demo workspace, and legacy `--local` serves it directly. Flows
// that need other data must commit a local file (DuckDB / Parquet / CSV)
// and reference it from inside `demo_project` — there is intentionally
// no env override so fixtures cannot point at an external warehouse.
const PROJECT_DIR = resolve(REPO_ROOT, "demo_project");
// `oxy start`'s own Postgres container (fixed port, crates/core/src/database/docker.rs).
// `oxy start` sets OXY_DATABASE_URL for its own process only, so the seed that
// follows it has to be told the same address.
const START_DATABASE_URL = "postgresql://postgres:postgres@localhost:15432/oxy";
const LOG_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..", ".logs");
const STARTUP_TIMEOUT_MS = 240_000;
// The seed compiles + promotes every seeded workspace (Demo plus the partner
// tenants, all pointed at demo_project/).
const SEED_TIMEOUT_MS = 600_000;
const POLL_INTERVAL_MS = 1_000;

// Pitfall #1: a stale `oxy` on PATH (often older than the workspace build)
// can refuse flags the workspace expects. Resolve in this order:
//   1. $OXY_BIN (explicit override)
//   2. <repo>/target/debug/oxy (fresh workspace build)
//   3. `oxy` on PATH
const DEBUG_OXY = resolve(REPO_ROOT, "target", "debug", "oxy");
const OXY_BIN =
  process.env.OXY_BIN && existsSync(process.env.OXY_BIN)
    ? process.env.OXY_BIN
    : existsSync(DEBUG_OXY)
      ? DEBUG_OXY
      : "oxy";

export interface BackendHandle {
  url: string;
  spawned: boolean;
  shutdown: () => Promise<void>;
}

export interface BackendOptions {
  /**
   * Which oxy backend mode to bring up. `cloud` (the default) spawns
   * `oxy start --enterprise` from the repo root and seeds `demo_project/` as
   * the Demo workspace; the runner then signs in on the public port (3000).
   * `local` spawns the legacy `oxy start --local --enterprise` from
   * `demo_project/` (auth-disabled, single workspace).
   */
  mode: BackendMode;
}

/**
 * Public URL the runner should drive. `OXY_BASE_URL` overrides. Both modes
 * default to the public port: enterprise mode signs in there with a real
 * session (session.ts) rather than using the auth-disabled internal port,
 * which carries neither `enforce_role` nor the ide proxy and whose
 * `/api/user` answers `null` to a cookie-less browser.
 */
export function resolveBaseUrl(_mode: BackendMode): string {
  return process.env.OXY_BASE_URL ?? DEFAULT_BASE_URL;
}

/**
 * Health-check URL. `OXY_HEALTH_URL` overrides; otherwise derive from the
 * resolved base URL.
 */
export function resolveHealthUrl(mode: BackendMode): string {
  return process.env.OXY_HEALTH_URL ?? `${resolveBaseUrl(mode)}/api/health`;
}

/**
 * Scope a flow's `goto:` path to a workspace when the target deployment needs
 * one.
 *
 * A flow says `goto:/automations` because that is what the surface is called.
 * In the single-workspace `--local` backend that path resolves directly; in a
 * cloud deployment the same surface lives under
 * `/<org>/workspaces/<workspace-id>/automations`, and the bare path silently
 * lands on the org home instead. The flow is not wrong — the deployment shape
 * is a property of where the run points, so the prefix belongs here next to
 * `OXY_BASE_URL`, not duplicated into every YAML file.
 *
 * Bare `/` is left alone deliberately: it already means "app root, route me",
 * which every deployment handles, and prefixing it would change the entry
 * point of flows that pass today.
 *
 * Every membership test below runs against the PATH ONLY, with any query string
 * or fragment stripped first. Comparing the whole target instead is a bug that
 * shipped here and cost a flow: `admin-airhouse-fleet`'s setup is
 * `goto:/dev-login?email=…&next=/admin/airhouse`, which is neither `/dev-login`
 * nor a `/dev-login/…` and so escaped the top-level list and got prefixed into
 * `/local/workspaces/<id>/dev-login?…`. The SPA rendered its fallback and the
 * flow timed out on its first locator after 30 seconds, reading exactly like a
 * broken admin page. Covered by backend.test.ts.
 */
export function applyPathPrefix(target: string): string {
  const prefix = process.env.OXY_PATH_PREFIX?.replace(/\/+$/, "");
  if (!prefix) return target;
  if (!target.startsWith("/")) return target; // absolute URL — caller means it
  if (target === "/") return target;
  const path = target.split(/[?#]/, 1)[0];
  if (path === prefix || path.startsWith(`${prefix}/`)) return target;
  if (TOP_LEVEL_SURFACES.some((p) => path === p || path.startsWith(`${p}/`))) return target;
  return `${prefix}${target}`;
}

/**
 * Surfaces that are NOT workspace-scoped, so the prefix must not reach them.
 *
 * The prefix exists because `goto:/automations` means a different URL in a
 * cloud deployment than in `--local`. But `/admin/workspace-health` means the
 * SAME url in both — it hangs off the app root, not off a workspace. Prefixing
 * it produces `/<org>/workspaces/<id>/admin/workspace-health`, which routes
 * nowhere; the SPA renders its fallback and the flow times out waiting for a
 * testid that was never going to appear. Three admin flows failed exactly that
 * way before this list existed, and none of the failures looked like a routing
 * problem — they looked like three unrelated broken pages.
 */
const TOP_LEVEL_SURFACES = [
  "/admin",
  "/partners",
  "/customer-apps",
  "/login",
  "/dev-login",
  "/invite",
  "/cli-auth"
];

const DEFAULT_BASE_URL = "http://localhost:3000";

export async function ensureBackend(opts: BackendOptions): Promise<BackendHandle> {
  const healthUrl = resolveHealthUrl(opts.mode);
  if (await isHealthy(healthUrl, 5_000)) {
    // Reused as-is: no respawn and no seed. A backend you started yourself
    // must already hold the Demo workspace and allow the flow identity to
    // sign in — see README "Running against a backend you started".
    console.log(`[backend] using running backend at ${healthUrl}`);
    return { url: healthUrl, spawned: false, shutdown: async () => {} };
  }

  const args = spawnArgs(opts.mode);
  const cwd = opts.mode === "cloud" ? REPO_ROOT : PROJECT_DIR;
  console.log(
    `[backend] not reachable at ${healthUrl}; starting \`${OXY_BIN} ${args.join(" ")}\` from ${cwd}`
  );
  mkdirSync(LOG_DIR, { recursive: true });
  const logPath = resolve(LOG_DIR, "backend.log");
  const out = openSync(logPath, "a");
  const proc = spawn(OXY_BIN, args, {
    cwd,
    env: opts.mode === "cloud" ? enterpriseServerEnv() : process.env,
    stdio: ["ignore", out, out],
    detached: false
  });

  // Race spawn against process exit so a misconfigured invocation surfaces
  // the actual error from the log instead of a 4-minute health-check timeout.
  const earlyExit = new Promise<never>((_, reject) => {
    proc.once("exit", (code, signal) => {
      reject(new Error(`oxy start exited early (code=${code} signal=${signal}) — see ${logPath}`));
    });
  });
  let ready: boolean;
  try {
    ready = await Promise.race([waitForHealthy(healthUrl, STARTUP_TIMEOUT_MS), earlyExit]);
  } catch (err) {
    proc.kill("SIGTERM");
    throw err;
  }
  if (!ready) {
    proc.kill("SIGTERM");
    throw new Error(
      `backend did not become healthy within ${STARTUP_TIMEOUT_MS}ms — see ${logPath}`
    );
  }

  console.log(`[backend] healthy after spawn (logs: ${logPath})`);

  if (opts.mode === "cloud") {
    try {
      seedDemoWorkspace();
    } catch (err) {
      await shutdownProc(proc);
      throw err;
    }
  }

  return {
    url: healthUrl,
    spawned: true,
    shutdown: () => shutdownProc(proc)
  };
}

function spawnArgs(mode: BackendMode): string[] {
  if (mode === "cloud") {
    // Enterprise mode, the production path. No `--clean`: the Postgres volume
    // is the one `just up` uses, and the seed below is idempotent, so wiping a
    // developer's data buys nothing.
    return ["start", "--enterprise"];
  }
  return ["start", "--local", "--enterprise"];
}

const emailList = (raw: string | undefined): string[] =>
  (raw ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
const sameEmail = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/**
 * The spawned server's environment: the caller's, plus the flow identity on the
 * dev-login allow-list so session.ts can sign in as it, and MINUS the flow
 * identity in `OXY_GLOBAL_ADMINS`: the server bootstraps `app_admins` from that
 * var, which would make the identity staff, and the SPA bounces staff off every
 * tenant workspace. Explicit `OXY_DEV_LOGIN_EMAILS` replaces the debug-build
 * persona roster — acceptable for a server the runner owns and shuts down.
 */
export function enterpriseServerEnv(env: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const email = flowEmail();
  if (env.OXY_OWNER && sameEmail(env.OXY_OWNER.trim(), email)) {
    throw new Error(
      `[backend] the flow identity ${email} is OXY_OWNER — a Global Owner is bounced off every ` +
        "workspace into /admin. Set OXY_FLOW_EMAIL to an address with no platform standing."
    );
  }
  const loginList = emailList(env.OXY_DEV_LOGIN_EMAILS);
  if (!loginList.some((e) => sameEmail(e, email))) loginList.push(email);
  const admins = emailList(env.OXY_GLOBAL_ADMINS).filter((e) => !sameEmail(e, email));
  return {
    ...env,
    OXY_DEV_LOGIN_EMAILS: loginList.join(","),
    OXY_GLOBAL_ADMINS: admins.join(",")
  };
}

/**
 * Seed `demo_project/` as the Demo workspace of the `local` org, with the flow
 * identity bound as its Owner, compiled + promoted, and the LLM keys this shell
 * exports stored as workspace secrets (cloud mode reads keys from the secrets
 * store, not the environment). Idempotent; re-points the Demo workspace at
 * `demo_project/` if `just up` had it on `examples/` (the next `just up`
 * points it back).
 *
 * Only reached right after the runner spawned `oxy start`, which OVERWRITES any
 * inherited `OXY_DATABASE_URL` with its own container's (start.rs), so the seed
 * targets that fixed loopback URL rather than whatever this shell exports — a
 * seed into a different database would exit 0 and leave every flow on
 * /onboarding.
 *
 * `OXY_GLOBAL_ADMINS` here reaches the SEED process only, where it binds Owners
 * of `local`: the caller's list (as `just up` binds it) plus the flow identity.
 */
export function seedEnv(env: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const email = flowEmail();
  const owners = emailList(env.OXY_GLOBAL_ADMINS).filter((e) => !sameEmail(e, email));
  return {
    ...env,
    OXY_DATABASE_URL: START_DATABASE_URL,
    OXY_GLOBAL_ADMINS: [...owners, email].join(",")
  };
}

function seedDemoWorkspace(): void {
  const args = ["seed", "--workspace-path", PROJECT_DIR, "--llm-keys"];
  console.log(`[backend] seeding the Demo workspace: \`${OXY_BIN} ${args.join(" ")}\``);
  const logPath = resolve(LOG_DIR, "seed.log");
  const out = openSync(logPath, "a");
  const res = spawnSync(OXY_BIN, args, {
    cwd: REPO_ROOT,
    env: seedEnv(),
    stdio: ["ignore", out, out],
    timeout: SEED_TIMEOUT_MS
  });
  if (res.status !== 0) {
    throw new Error(
      `oxy seed failed (status=${res.status} signal=${res.signal ?? "none"}) — see ${logPath}`
    );
  }
  console.log(`[backend] Demo workspace seeded from demo_project/ (logs: ${logPath})`);
}

async function isHealthy(url: string, timeoutMs: number): Promise<boolean> {
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
    return res.ok;
  } catch {
    return false;
  }
}

async function waitForHealthy(url: string, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await isHealthy(url, 2_000)) return true;
    await sleep(POLL_INTERVAL_MS);
  }
  return false;
}

async function shutdownProc(proc: ChildProcess): Promise<void> {
  if (proc.exitCode !== null || proc.killed) return;
  proc.kill("SIGTERM");
  await Promise.race([new Promise<void>((r) => proc.once("exit", () => r())), sleep(5_000)]);
  if (proc.exitCode === null && !proc.killed) proc.kill("SIGKILL");
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
