/**
 * A custom app that tries to send what it read somewhere else
 * (`hostile-page.js` is its page), and the loopback servers it is played
 * against: for the promote checks' proof in a REAL Chromium
 * (`checks/browser-confine.real.test.ts`). Test support only.
 *
 * `app` stands in for the deployment (`http://localhost:<port>`); `other` is
 * everything that is not it, on the same machine (`http://127.0.0.1:<port>`:
 * another host, and through `localhost:<that port>` the same host on another
 * port). Both record every request, upgrade and connection they receive, and a
 * raw TCP and a UDP listener record anything that is not HTTP. Nothing here
 * leaves the loopback interface.
 */

import { createSocket } from "node:dgram";
import { readFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { createServer as createTcpServer } from "node:net";

export const HOSTILE_WS = "00000000-0000-0000-0000-0000000000b1";
export const HOSTILE_BASE = "/customer-apps/acme/store/";

/** What the listeners saw. */
export interface Recorded {
  hits: string[];
  port: number;
  close(): Promise<void>;
}

type Answer = (req: IncomingMessage, res: ServerResponse, body: string) => void;

/** An HTTP server that records every request (`"<METHOD> <url>"`) and every upgrade. */
async function httpServer(routes: (path: string) => Answer | undefined): Promise<Recorded> {
  const hits: string[] = [];
  const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk: Buffer) => {
      body += chunk.toString("utf8");
    });
    req.on("end", () => {
      const url = req.url ?? "";
      hits.push(`${req.method} ${url}`);
      const answer = routes(url.split("?")[0] ?? "");
      if (answer) return answer(req, res, body);
      res.writeHead(200, { "content-type": "text/plain", "access-control-allow-origin": "*" });
      res.end("ok");
    });
  });
  server.on("upgrade", (req, socket) => {
    hits.push(`UPGRADE ${req.url}`);
    socket.destroy();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return {
    hits,
    port: typeof address === "object" && address ? address.port : 0,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      })
  };
}

/** A TCP listener that records each connection: what a TURN server over TCP would see. */
async function tcpListener(): Promise<Recorded> {
  const hits: string[] = [];
  const server = createTcpServer((socket) => {
    hits.push("TCP connection");
    socket.on("error", () => undefined);
    socket.destroy();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return {
    hits,
    port: typeof address === "object" && address ? address.port : 0,
    close: () => new Promise((resolve) => server.close(() => resolve()))
  };
}

/** A UDP socket that records each datagram: what a STUN server, or QUIC, would see. */
async function udpListener(): Promise<Recorded> {
  const hits: string[] = [];
  const socket = createSocket("udp4");
  socket.on("message", (message) => hits.push(`UDP ${message.length} bytes`));
  await new Promise<void>((resolve) => socket.bind(0, "127.0.0.1", resolve));
  return {
    hits,
    port: socket.address().port,
    close: () => new Promise((resolve) => socket.close(() => resolve()))
  };
}

/** Everything the hostile app is played against. */
export interface HostileWorld {
  /** The deployment: `http://localhost:<port>`. */
  origin: string;
  pageUrl: string;
  app: Recorded;
  /** Another origin's HTTP server, raw TCP listener and UDP listener. */
  other: Recorded;
  tcp: Recorded;
  udp: Recorded;
  /** What the page said it saw, once it has (`report?…` on the app). */
  report(): Record<string, unknown> | undefined;
  /** Everything that arrived anywhere but the app's origin. */
  leaked(): string[];
  close(): Promise<void>;
}

/** The page's own script (`hostile-page.js`), told where "another origin" is on this machine. */
function hostilePage(build: string, ports: { other: number; tcp: number; udp: number }): string {
  const script = readFileSync(new URL("./hostile-page.js", import.meta.url), "utf8");
  const app = JSON.stringify({ environment: "staging", buildId: build });
  const given = JSON.stringify({ ...ports, ws: HOSTILE_WS });
  return [
    "<!doctype html><title>app</title><body>",
    `<script>window.__OXY_APP__ = ${app}; window.__HOSTILE__ = ${given};</script>`,
    `<script>${script}</script>`,
    "</body>"
  ].join("");
}

/** Start the app, the other origin and the two raw listeners. */
export async function startHostileWorld(build: string): Promise<HostileWorld> {
  const other = await httpServer(() => undefined);
  const tcp = await tcpListener();
  const udp = await udpListener();
  let reported: Record<string, unknown> | undefined;
  const redirect = (to: string): Answer => {
    return (_req, res) => {
      res.writeHead(302, { location: to });
      res.end();
    };
  };
  const document: Answer = (_req, res) => {
    res.writeHead(200, { "content-type": "text/html" });
    res.end(hostilePage(build, { other: other.port, tcp: tcp.port, udp: udp.port }));
  };
  const app = await httpServer((path) => {
    if (path === HOSTILE_BASE) return document;
    if (path === `/api/projects/${HOSTILE_WS}/query`) {
      // The rows are the app's only for a request that carries the checker.
      return (req, res) => {
        const signed = req.headers["x-api-key"] === "oxysvc_x";
        res.writeHead(signed ? 200 : 401, { "content-type": "application/json" });
        res.end(signed ? '{"rows":[["payroll",12900]]}' : "{}");
      };
    }
    if (path === "/api/redirect-away") return redirect(`http://127.0.0.1:${other.port}/redirected`);
    // Signed, then twice redirected: to the app's origin, and from there away.
    if (path === `${HOSTILE_BASE}hop`) return redirect("/api/second-hop");
    if (path === "/api/second-hop") return redirect(`http://127.0.0.1:${other.port}/second-hop`);
    if (path === `${HOSTILE_BASE}report`) {
      return (req, res) => {
        const query = (req.url ?? "").split("?")[1] ?? "";
        reported = JSON.parse(decodeURIComponent(query)) as Record<string, unknown>;
        res.writeHead(204);
        res.end();
      };
    }
    return undefined;
  });
  const origin = `http://localhost:${app.port}`;
  return {
    origin,
    pageUrl: `${origin}${HOSTILE_BASE}`,
    app,
    other,
    tcp,
    udp,
    report: () => reported,
    leaked: () => [...other.hits, ...tcp.hits, ...udp.hits],
    close: async () => {
      await Promise.all([app.close(), other.close(), tcp.close(), udp.close()]);
    }
  };
}
