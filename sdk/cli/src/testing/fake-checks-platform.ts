/**
 * A stand-in for the platform's build-checks routes, on a loopback port the
 * OS picks: the ones a runner drives (`claim`, `functions/settle`,
 * `stylesheets`, `report`, `files/{name}`) and the app listing, so a test can
 * prove which were called, in what order, and with exactly which body.
 *
 * A kept file (`PUT …/files/{name}?attempt=…&environment=…`) is one route,
 * `PUT /files`, whatever its name: {@link FakePlatform.files} lists what was
 * kept, each with its name, its content type and its bytes.
 *
 * A real socket rather than a stubbed `fetch`: `oxyc checks run --build` is
 * what a GitHub Actions runner executes against a deployment, and its
 * credential header, its repeats of a failed request and its request bodies
 * are the contract. Every request is recorded before it is answered.
 */

import { createServer, type IncomingMessage, type Server } from "node:http";
import type { AddressInfo } from "node:net";

export const APP_ID = "a1a1a1a1-2222-3333-4444-555555555555";
export const BUILD_ROW = "b2b2b2b2-2222-3333-4444-555555555555";
export const BUILD_ID = "4c1e9a2f";
export const WORKSPACE_ID = "c3c3c3c3-2222-3333-4444-555555555555";

/** One request the platform received. */
export interface Call {
  /** `METHOD /path`, the path without the checks base when it is under it. */
  route: string;
  path: string;
  headers: Record<string, string | string[] | undefined>;
  /** The JSON body, or `undefined` when there was none or it was a file's bytes. */
  body: unknown;
  /** A `PUT …/files/{name}`: the file as it was sent. */
  file?: SentFile;
}

/** One file a runner sent to be kept. */
export interface SentFile {
  name: string;
  /** The query's `attempt` and `environment`, as sent. */
  attempt: string | null;
  environment: string | null;
  contentType: string;
  bytes: Buffer;
}

export interface Answer {
  status: number;
  body?: unknown;
}

/** Answers one route; `nth` counts that route's calls from 1. */
export type Handler = (call: Call, nth: number) => Answer;

export interface FakePlatform {
  /** The base URL: `--target`. */
  url: string;
  /** The page a claim names, on this origin. */
  pageUrl: string;
  calls: Call[];
  /** Replace the answer of one route (`POST /claim`, `GET /stylesheets`, …). */
  on(route: string, handler: Handler): void;
  /** The routes called, in order. */
  routes(): string[];
  /** The bodies one route received, in order. */
  bodies(route: string): unknown[];
  /** The files the platform kept (answered 2xx), in order. */
  files(): SentFile[];
  close(): Promise<void>;
}

const BASE = `/api/customer-apps/${APP_ID}/builds/${BUILD_ROW}/checks`;

/** The claim a healthy attempt answers; `open` and the rest are overridable. */
export function claimOf(pageUrl: string, overrides: Record<string, unknown> = {}): unknown {
  return {
    app: { id: APP_ID, org_slug: "acme", slug: "store", workspace_id: WORKSPACE_ID },
    build: { id: BUILD_ROW, build_id: BUILD_ID, sdk_version: "2.31.0" },
    environment: "staging",
    attempt: 2,
    serving: true,
    url: pageUrl,
    sdk_floor: "2.30.0",
    open: ["functions", "loads", "design_system"],
    ...overrides
  };
}

/** `PUT /files/<name>?attempt=2&environment=staging`, read apart; `undefined` for any other route. */
function fileOf(
  method: string | undefined,
  local: string,
  contentType: string,
  bytes: Buffer
): SentFile | undefined {
  if (method !== "PUT" || !local.startsWith("/files/")) return undefined;
  const url = new URL(local, "http://fake.invalid");
  return {
    name: decodeURIComponent(url.pathname.slice("/files/".length)),
    attempt: url.searchParams.get("attempt"),
    environment: url.searchParams.get("environment"),
    contentType,
    bytes
  };
}

/** One request as the platform records it: its route, and its body as JSON or as a file's bytes. */
function callOf(req: IncomingMessage, bytes: Buffer): Call {
  const path = req.url ?? "/";
  const local = path.startsWith(BASE) ? path.slice(BASE.length) || "/" : path;
  const file = fileOf(req.method, local, String(req.headers["content-type"] ?? ""), bytes);
  const text = file ? "" : bytes.toString("utf8");
  return {
    route: file ? "PUT /files" : `${req.method} ${local}`,
    path,
    headers: req.headers,
    body: text === "" ? undefined : JSON.parse(text),
    file
  };
}

export async function startFakePlatform(): Promise<FakePlatform> {
  const calls: Call[] = [];
  const kept: SentFile[] = [];
  const counts = new Map<string, number>();
  const handlers = new Map<string, Handler>();
  let pageUrl = "";

  const defaults: Record<string, Handler> = {
    "POST /claim": () => ({ status: 200, body: claimOf(pageUrl) }),
    "POST /functions/settle": () => ({
      status: 200,
      body: { done: true, status: "passed", summary: "3 of 3 passed" }
    }),
    "GET /stylesheets": () => ({
      status: 200,
      body: {
        sources: [
          { file: "index.html", css: "<html><style>.app{color:var(--foreground)}</style></html>" },
          { file: "assets/index-abc.css", css: ".app{background:var(--card)}" }
        ]
      }
    }),
    "POST /report": () => ({ status: 200, body: { state: "running", open: [] } }),
    "PUT /files": () => ({ status: 204 })
  };

  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const call = callOf(req, Buffer.concat(chunks));
      const { route, file } = call;
      calls.push(call);
      const nth = (counts.get(route) ?? 0) + 1;
      counts.set(route, nth);
      const handler = handlers.get(route) ?? defaults[route];
      const answer: Answer = handler
        ? handler(call, nth)
        : { status: 404, body: { code: "not_found", message: `no route ${route}` } };
      if (file && answer.status >= 200 && answer.status < 300) kept.push(file);
      res.writeHead(answer.status, { "content-type": "application/json" });
      res.end(answer.body === undefined ? "" : JSON.stringify(answer.body));
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  pageUrl = `${url}/customer-apps/acme/store/`;

  return {
    url,
    pageUrl,
    calls,
    on: (route, handler) => {
      handlers.set(route, handler);
    },
    routes: () => calls.map((call) => call.route),
    bodies: (route) => calls.filter((call) => call.route === route).map((call) => call.body),
    files: () => [...kept],
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((error) => (error ? reject(error) : resolve()));
      })
  };
}
