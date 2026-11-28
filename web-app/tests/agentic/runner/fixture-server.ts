// A loopback HTTP responder for flows whose backend work calls a third-party
// API. It answers fixed GET routes with committed JSON files and nothing else:
// no writes, no proxying, no network egress.
//
// Why it exists: `airway-pipeline-run` ran the demo `weather.airway.yml`,
// whose source calls the live `api.open-meteo.com`. When the CI runner could
// not reach that host (run 46124368691), airway retried, the run never
// finished, and the flow timed out waiting for "done". The flow now runs
// `examples/pipelines/weather_fixture.airway.yml` instead: the same
// `kind: weather` connector and the same extract -> normalize -> load path,
// with `base_url` pointed here.
//
// The port and path are written into that pipeline file, so they are fixed
// constants, not configuration: airway's `{{ … }}` templating renders only
// run-request variables, which the UI's Run button never sends, so the port
// cannot come from the environment. The runner starts this only when a
// selected flow is in FIXTURE_FLOWS, and then whether or not it spawned the
// backend (CI passes --no-auto-backend), because the backend process makes the
// request, not the browser.

import { readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const FIXTURES_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..", "fixtures");

/** Loopback only. A literal IP, not `localhost`, so an IPv6 resolution cannot miss it. */
export const FIXTURE_HOST = "127.0.0.1";
/** Must match `base_url` in `examples/pipelines/weather_fixture.airway.yml`. */
export const FIXTURE_PORT = 18744;

/** Route path (query ignored) -> committed fixture file under `fixtures/`. */
export const FIXTURE_ROUTES: Readonly<Record<string, string>> = {
  "/open-meteo/v1/forecast": "open-meteo/forecast.json"
};

/**
 * Flows (file stem) whose backend work reads a fixture route. Only a run that
 * selects one of these binds the port, so any other flow can run alongside it
 * on the same machine.
 */
export const FIXTURE_FLOWS: ReadonlySet<string> = new Set(["airway-pipeline-run"]);

const flowStem = (file: string) => basename(file).replace(/\.flow\.test\.ya?ml$/, "");

export interface FixtureServerHandle {
  /** `null` when no selected flow needed the responder and nothing was bound. */
  url: string | null;
  shutdown: () => Promise<void>;
}

/** A handle for a responder that is actually listening. */
export type StartedFixtureServer = FixtureServerHandle & { url: string };

function loadBodies(): Map<string, string> {
  const bodies = new Map<string, string>();
  for (const [route, file] of Object.entries(FIXTURE_ROUTES)) {
    const body = readFileSync(resolve(FIXTURES_DIR, file), "utf8");
    JSON.parse(body); // a broken fixture fails at startup, not mid-flow
    bodies.set(route, body);
  }
  return bodies;
}

function createFixtureServer(bodies: Map<string, string>): Server {
  return createServer((req, res) => {
    const path = new URL(req.url ?? "/", `http://${FIXTURE_HOST}`).pathname;
    const body = bodies.get(path);
    if (req.method !== "GET" || body === undefined) {
      res.writeHead(req.method === "GET" ? 404 : 405, { "content-type": "text/plain" });
      res.end(`fixture server: no ${req.method} route for ${path}\n`);
      return;
    }
    res.writeHead(200, { "content-type": "application/json" });
    res.end(body);
  });
}

function listen(server: Server, port: number): Promise<void> {
  return new Promise((ok, fail) => {
    server.once("error", fail);
    server.listen(port, FIXTURE_HOST, () => {
      server.off("error", fail);
      ok();
    });
  });
}

/**
 * Start the responder on `port` (default {@link FIXTURE_PORT}). If the port is
 * already taken, throw, whoever holds it. Reusing another runner's responder is
 * not safe: that runner closes it when its own flows finish, and a pipeline run
 * here would then fail with a connection error no log explains. So two runs of
 * a flow in FIXTURE_FLOWS on one machine conflict; other flows never bind it.
 */
export async function ensureFixtureServer(port = FIXTURE_PORT): Promise<StartedFixtureServer> {
  const bodies = loadBodies();
  const server = createFixtureServer(bodies);
  try {
    await listen(server, port);
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code !== "EADDRINUSE") throw err;
    throw new Error(
      `[fixtures] ${FIXTURE_HOST}:${port} is already in use (another agentic runner, or ` +
        "something else). Stop it and rerun: examples/pipelines/weather_fixture.airway.yml " +
        "reads its weather from this port."
    );
  }
  const address = server.address();
  const boundPort = typeof address === "object" && address ? address.port : port;
  const url = `http://${FIXTURE_HOST}:${boundPort}`;
  console.log(`[fixtures] serving ${Object.keys(FIXTURE_ROUTES).join(", ")} on ${url}`);
  return {
    url,
    shutdown: () => new Promise<void>((ok) => server.close(() => ok()))
  };
}

/**
 * Start the responder only when a selected flow is in {@link FIXTURE_FLOWS};
 * otherwise return a no-op handle and bind nothing.
 */
export async function ensureFixtureServerFor(
  flows: ReadonlyArray<{ file: string }>,
  port = FIXTURE_PORT
): Promise<FixtureServerHandle> {
  if (!flows.some((f) => FIXTURE_FLOWS.has(flowStem(f.file)))) {
    return { url: null, shutdown: async () => {} };
  }
  return ensureFixtureServer(port);
}
