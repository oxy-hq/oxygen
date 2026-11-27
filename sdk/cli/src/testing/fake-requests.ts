/**
 * How one scripted request plays out in the stand-in for Chromium
 * (`fake-playwright.ts`): it is put through the check's route handler, and
 * what the handler decided is played the way Chromium plays it.
 *
 * REDIRECTS BEHAVE AS CHROMIUM'S DO UNDER PLAYWRIGHT, which is the point of
 * scripting them (each was seen in a real headless Chromium):
 *
 * - a request the handler CONTINUED is followed through its redirect without
 *   the handler being asked again, carrying the headers it was continued with;
 * - a request sent from outside the page (`route.fetch`) follows a redirect
 *   itself, with the same headers, unless `maxRedirects` is 0;
 * - a redirect HANDED BACK to the page (`route.fulfill`) is followed with the
 *   page's own headers and without the handler: always for the document, and
 *   for anything else only on the same origin. To another origin it fails
 *   (`net::ERR_FAILED`) and nothing is sent.
 *
 * A WEBSOCKET is no request: the route handler is never asked about one. It
 * goes to the context's socket handler when the check set one
 * (`routeWebSocket`), and is opened by the browser, asking no one, when it
 * did not.
 */

import type {
  PwApiResponse,
  PwRequest,
  PwResponse,
  PwRoute,
  PwWebSocketRoute
} from "../checks/playwright.js";

/** A request the page makes once its document has answered. */
export interface ScriptedRequest {
  method?: string;
  url: string;
  postData?: string;
  /** What the server answers; 200 when neither this nor `failure` nor `redirectTo` is set. */
  status?: number;
  /** The server does not answer: the error a request to it fails with. */
  failure?: string;
  /** The server answers a redirect to this URL (302 unless `status` says otherwise). */
  redirectTo?: string;
  /** How long after the one before it (or the document) the page makes it. */
  afterMs?: number;
  /** The page went away while the request was in flight: answering it throws. */
  gone?: boolean;
  /** The page itself cancelled the request in flight: it fails as aborted, whatever was decided. */
  cancelled?: boolean;
}

/** One request, and what became of it. */
export interface Sent {
  /** Which context it was made in, by creation order. */
  context: number;
  method: string;
  url: string;
  /**
   * What the check's handler did with it. `followed`: a redirect hop, which
   * the handler is never asked about.
   */
  decision: "continued" | "aborted" | "fulfilled" | "followed";
  /** The headers it went out with; the page's own when the handler set none. */
  headers: Record<string, string>;
  /** The redirect limit, for a request the handler sent from outside the page. */
  maxRedirects?: number;
}

/** The headers every scripted request starts with. */
const OWN_HEADERS = { "user-agent": "fake-chromium" };

/** What Chromium calls each way a handler can fail a request. */
const ABORTS: Record<string, string> = {
  blockedbyclient: "net::ERR_BLOCKED_BY_CLIENT",
  connectionrefused: "net::ERR_CONNECTION_REFUSED",
  connectionreset: "net::ERR_CONNECTION_RESET",
  namenotresolved: "net::ERR_NAME_NOT_RESOLVED",
  timedout: "net::ERR_TIMED_OUT",
  failed: "net::ERR_FAILED"
};

/** What the check listens for on one page. */
export type Listeners = {
  pageerror: ((error: Error) => void)[];
  response: ((response: PwResponse) => void)[];
  requestfailed: ((request: PwRequest) => void)[];
};

/** How one scripted request ended. */
export interface Ended {
  /** Why it failed, when it did. */
  failure?: string;
  /** Where it finally answered from, after any redirect. */
  url: string;
  status: number;
}

/** A context's one route handler and its one socket handler, once the check has set them. */
export interface Routed {
  handler?: (route: PwRoute) => Promise<void>;
  socket?: (socket: PwWebSocketRoute) => void | Promise<void>;
}

/** One WebSocket a page asked for, and what became of it. */
export interface Socket {
  context: number;
  url: string;
  /**
   * `connected`: nothing routed it, so the browser opened it, asking no one.
   * `closed`: the check's handler ended it, and nothing was sent. `held`: the
   * handler took it and did nothing, which leaves the page waiting.
   */
  decision: "connected" | "closed" | "held";
}

/** Play one WebSocket the page constructs through the context's socket handler. */
export async function openScripted(routed: Routed, context: number, url: string): Promise<Socket> {
  if (!routed.socket) return { context, url, decision: "connected" };
  const socket: Socket = { context, url, decision: "held" };
  await routed.socket({
    url: () => url,
    close: async () => {
      socket.decision = "closed";
    }
  });
  return socket;
}

/** What one page's requests are played through. */
export interface Wire {
  /** Which context the page is in, by creation order. */
  context: number;
  routed: Routed;
  /** The page's own listeners. */
  listeners: Listeners;
  /** Where each request, and what became of it, is recorded. */
  sent: Sent[];
}

/** What the check's handler made of a request. An object: the handler sets it from inside a callback. */
interface Made {
  decision: Sent["decision"];
  headers: Record<string, string>;
  maxRedirects?: number;
  abort?: string;
  handedBack?: PwApiResponse;
}

/** One scripted request on its way through a page. */
interface Flight {
  wire: Wire;
  scripted: ScriptedRequest;
  method: string;
  /** What the server answers when the request reaches it. */
  status: number;
  made: Made;
}

function requestOf(
  scripted: Pick<ScriptedRequest, "url" | "method" | "postData">,
  failure?: string
): PwRequest {
  return {
    url: () => scripted.url,
    method: () => scripted.method ?? "GET",
    headers: () => ({ ...OWN_HEADERS }),
    postData: () => scripted.postData ?? null,
    failure: () => (failure === undefined ? null : { errorText: failure })
  };
}

/** A response as the page's listeners and `goto` see it. */
export function answer(url: string, status: number): PwResponse {
  return { url: () => url, status: () => status, headers: () => ({}) };
}

function apiResponse(status: number, location?: string): PwApiResponse {
  const headers: Record<string, string> = location === undefined ? {} : { location };
  return { status: () => status, headers: () => headers };
}

/** Tell the page's listeners a request failed. */
function fail(wire: Wire, url: string, method: string, errorText: string): void {
  const failed = requestOf({ url, method }, errorText);
  for (const fn of wire.listeners.requestfailed) fn(failed);
}

/** Tell the page's listeners a response arrived. */
function answered(wire: Wire, url: string, status: number): void {
  for (const fn of wire.listeners.response) fn(answer(url, status));
}

/** Record the hop a redirect is followed with: one the handler is never asked about. */
function follow(flight: Flight, headers: Record<string, string>): void {
  flight.wire.sent.push({
    context: flight.wire.context,
    method: flight.method,
    url: flight.scripted.redirectTo as string,
    decision: "followed",
    headers
  });
}

/** The route the check's handler is handed: each call records what the handler decided. */
function routeOf(flight: Flight): PwRoute {
  const { scripted, status, made } = flight;
  const stillThere = (call: string) => {
    if (scripted.gone) {
      throw new Error(`route.${call}: Target page, context or browser has been closed`);
    }
  };
  return {
    request: () => requestOf(scripted),
    continue: async (opts) => {
      stillThere("continue");
      made.headers = opts?.headers ?? OWN_HEADERS;
    },
    abort: async (code) => {
      stillThere("abort");
      made.decision = "aborted";
      made.abort = ABORTS[code ?? "failed"] ?? ABORTS.failed;
    },
    fetch: async (opts) => {
      made.headers = opts.headers;
      made.maxRedirects = opts.maxRedirects;
      if (scripted.failure !== undefined) throw new Error(scripted.failure);
      if (scripted.redirectTo === undefined) return apiResponse(status);
      if (opts.maxRedirects === 0) return apiResponse(status, scripted.redirectTo);
      follow(flight, opts.headers);
      return apiResponse(200);
    },
    fulfill: async ({ response }) => {
      stillThere("fulfill");
      made.decision = "fulfilled";
      made.handedBack = response;
    }
  };
}

/** The handler failed the request. */
function afterAbort(flight: Flight): Ended {
  const { wire, scripted, method, made } = flight;
  const failure = made.abort ?? "net::ERR_FAILED";
  fail(wire, scripted.url, method, failure);
  return { failure, url: scripted.url, status: 0 };
}

/** The handler let the request go: the server answers, or does not. */
function afterContinue(flight: Flight): Ended {
  const { wire, scripted, method, status, made } = flight;
  if (scripted.failure !== undefined) {
    fail(wire, scripted.url, method, scripted.failure);
    return { failure: scripted.failure, url: scripted.url, status: 0 };
  }
  answered(wire, scripted.url, status);
  if (scripted.redirectTo === undefined) return { url: scripted.url, status };
  // Followed without the handler, with the headers it was continued with.
  follow(flight, made.headers);
  answered(wire, scripted.redirectTo, 200);
  return { url: scripted.redirectTo, status: 200 };
}

/** The handler handed a response back to the page, which follows its redirect itself. */
function afterFulfill(flight: Flight, isDocument: boolean): Ended {
  const { wire, scripted, method, made } = flight;
  const back = made.handedBack as PwApiResponse;
  answered(wire, scripted.url, back.status());
  const location = back.headers().location;
  if (location === undefined) return { url: scripted.url, status: back.status() };
  if (!isDocument && new URL(location).origin !== new URL(scripted.url).origin) {
    fail(wire, location, method, "net::ERR_FAILED");
    return { url: scripted.url, status: back.status() };
  }
  follow(flight, OWN_HEADERS);
  answered(wire, location, 200);
  return { url: location, status: 200 };
}

/** Put one request through the check's handler, then play out what came of it. */
export async function sendScripted(
  wire: Wire,
  scripted: ScriptedRequest,
  isDocument: boolean
): Promise<Ended> {
  const flight: Flight = {
    wire,
    scripted,
    method: scripted.method ?? "GET",
    status: scripted.status ?? (scripted.redirectTo ? 302 : 200),
    made: { decision: "continued", headers: OWN_HEADERS }
  };
  await wire.routed.handler?.(routeOf(flight));
  const { decision, headers, maxRedirects } = flight.made;
  if (scripted.gone) return { url: scripted.url, status: 0 };
  wire.sent.push({
    context: wire.context,
    method: flight.method,
    url: scripted.url,
    decision,
    headers,
    ...(maxRedirects === undefined ? {} : { maxRedirects })
  });

  if (scripted.cancelled) {
    fail(wire, scripted.url, flight.method, "net::ERR_ABORTED");
    return { failure: "net::ERR_ABORTED", url: scripted.url, status: 0 };
  }
  if (decision === "aborted") return afterAbort(flight);
  if (decision === "continued") return afterContinue(flight);
  return afterFulfill(flight, isDocument);
}
