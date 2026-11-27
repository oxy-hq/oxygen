/**
 * What the promote checks' browser loads, and what each of its requests may
 * carry.
 *
 * The page is a build on an app's staging, loaded on the path URL
 * (`{origin}/customer-apps/{org}/{app}/`) as the platform's checker. The code
 * it runs is the build under test, a tenant's own, on the deployment's origin
 * beside the whole API. Two rules hold for every request it makes
 * ({@link Treatment}):
 *
 * NOTHING IS SENT TO ANOTHER ORIGIN. The page can read the app's data as the
 * checker (below), so it is given nowhere to send what it read: a request to
 * any origin but the app's own is REFUSED in the browser, never sent, and
 * recorded, whatever its method and whatever it is for: a script, a font, an
 * image, a `fetch`, a beacon, a frame, a navigation. On staging and on
 * production alike. A loopback, private, link-local or internal-only address
 * is refused by a rule of its own, so it stays refused whatever becomes of
 * the first.
 *
 * ON THE APP'S ORIGIN, a request is one of three:
 *
 * - SIGNED with the checker's credential and `X-Oxy-App-Env: staging`: a read
 *   (`GET`/`HEAD`) under the app's base path (the document, its assets) and
 *   the runtime's ready beacon; and the app's own data reads, `POST
 *   /api/projects/{its workspace}/query` and `/semantic-query`, which also
 *   name the app (`x-oxy-app`) so the server can hold them to it; and the
 *   shell's bootstrap, `GET /api/projects/{its workspace}/shell-context`,
 *   named the same way, which the server answers without a viewer, so the
 *   pictures carry the app's chrome as people see it. On PRODUCTION the same
 *   requests are signed and no others, with no environment header at all:
 *   production is what the URL serves unnamed.
 * - PASSED as the page sent it, unsigned: any other read, as an anonymous
 *   visitor's would go.
 * - REFUSED, never sent, and recorded: everything else. A form post, a `/fn`
 *   call, an `/api` write. The checker reads a build; nothing it loads may
 *   change anything.
 *
 * What never leaves the browser (`data:`, `blob:`, `about:`) is left to it.
 *
 * This judges the requests the check is shown. What the browser does without
 * showing one (a redirect it follows, a socket, a peer connection) is held to
 * the same origin by `browser-confine.ts`.
 *
 * The platform holds the same line on its side: it answers the checker on the
 * staging page, its assets, the beacon and those two read endpoints, and
 * refuses everything else.
 */

import { BlockList, isIP } from "node:net";

/** One viewport the page is loaded at. */
export interface Viewport {
  name: "desk" | "phone";
  width: number;
  height: number;
  mobile: boolean;
}

/** Desk 1440 and phone 390, the widths every design review uses. */
export const VIEWPORTS: readonly [Viewport, Viewport] = [
  { name: "desk", width: 1440, height: 900, mobile: false },
  { name: "phone", width: 390, height: 844, mobile: true }
];

/** How long the page has to send `oxy-app-ready`, per viewport. */
export const READY_WAIT_MS = 30_000;

/** The one environment a page is asked for without naming it. */
const PRODUCTION = "production";

export interface LoadsTarget {
  /** The app's base on the path URL. */
  url: string;
  /** The one origin that receives the credential. */
  origin: string;
  /** The app's base path on that origin: only reads under it get `headers`. */
  basePath: string;
  /** The checker's credential and the environment header (none on production), lower-case names. */
  headers: Record<string, string>;
  /** `headers` and `x-oxy-app`, for the app's own data reads. */
  dataHeaders: Record<string, string>;
  /** The two data-plane read paths of the app's workspace; none when it is unknown. */
  dataPaths: readonly string[];
  /** The shell's bootstrap of the app's workspace, read with `dataHeaders`; none when it is unknown. */
  shellPath: string | null;
  /** The instant the page's `Date` starts at (`page-clock.ts`); the page's own clock when absent. */
  clockAt?: number;
  /** The environment the page must report. */
  environment: string;
  /** The build that environment serves: the one being checked. */
  expectedBuild: string;
  waitMs: number;
}

export interface LoadsTargetInput {
  /** The page to open, as the platform named it. */
  pageUrl: string;
  /** The deployment the command was pointed at: the only origin the credential may reach. */
  deployment: string;
  /** The credential as request headers (an API key, or a bearer). */
  credential: Record<string, string>;
  appId: string;
  /** The app's workspace, when the platform said which; without it no data read is signed. */
  workspaceId?: string;
  environment: string;
  expectedBuild: string;
  /** The instant the page's `Date` starts at, the same for every page of a run (`page-clock.ts`). */
  clockAt?: number;
  /** How long the page has to become ready; {@link READY_WAIT_MS} unless a test shortens it. */
  waitMs?: number;
}

/**
 * The target for one build's page. Throws, with the reason, when the page is
 * not somewhere the credential may go: another origin than the deployment's,
 * or a URL with no base path (every request on the origin would be "under" it).
 */
export function loadsTarget(input: LoadsTargetInput): LoadsTarget {
  let page: URL;
  try {
    page = new URL(input.pageUrl);
  } catch {
    throw new Error(`the platform named a page that is not a URL: ${input.pageUrl}`);
  }
  const deployment = new URL(input.deployment).origin;
  if (page.origin !== deployment) {
    throw new Error(
      `the platform named a page on ${page.origin}, but this run targets ${deployment}: the checker's credential is sent to the target only`
    );
  }
  if (page.pathname === "/" || !page.pathname.endsWith("/")) {
    throw new Error(
      `the page ${input.pageUrl} is not an app's base path (…/customer-apps/<org>/<app>/), so there is no base to hold the checker's credential to`
    );
  }
  // Production is what the URL serves when no environment is named, so none is:
  // the header would ask for an environment by name. Each signed read asks for a
  // fresh copy instead, so a cached page of the build before is not what is judged.
  const headers: Record<string, string> =
    input.environment === PRODUCTION
      ? { "cache-control": "no-cache" }
      : { "x-oxy-app-env": input.environment };
  for (const [name, value] of Object.entries(input.credential)) headers[name.toLowerCase()] = value;
  const workspace = input.workspaceId;
  return {
    url: page.href,
    origin: page.origin,
    basePath: page.pathname,
    headers,
    dataHeaders: { ...headers, "x-oxy-app": input.appId },
    dataPaths: workspace
      ? [`/api/projects/${workspace}/query`, `/api/projects/${workspace}/semantic-query`]
      : [],
    shellPath: workspace ? `/api/projects/${workspace}/shell-context` : null,
    clockAt: input.clockAt,
    environment: input.environment,
    expectedBuild: input.expectedBuild,
    waitMs: input.waitMs ?? READY_WAIT_MS
  };
}

/** What one request of the page gets. */
export type Treatment =
  /** Sent with these headers set over the page's own. */
  | { kind: "sign"; headers: Record<string, string> }
  /** Sent as the page sent it. */
  | { kind: "pass" }
  /** Nothing is sent anywhere: the browser answers it from what it already holds. */
  | { kind: "local" }
  | Refusal;

/**
 * Not sent; `what` (`"<METHOD> <path>"`, or `"<METHOD> <url> (<why>)"` off the
 * app's origin) is recorded. `why`: a `write` on the app's origin, or
 * `off-origin`, where nothing is sent whatever it is.
 */
export interface Refusal {
  kind: "refuse";
  what: string;
  why: "write" | "off-origin";
}

const PASS: Treatment = { kind: "pass" };
const LOCAL: Treatment = { kind: "local" };

/** The schemes whose URLs name something the browser already holds. */
const LOCAL_SCHEMES = ["data:", "blob:", "about:"];

function refuseWrite(method: string, path: string): Refusal {
  return { kind: "refuse", why: "write", what: `${method} ${path}` };
}

function refuseElsewhere(method: string, url: string, why: string): Refusal {
  return { kind: "refuse", why: "off-origin", what: `${method} ${url} (${why})` };
}

/** The refusal of anything to a host that is not the app's origin. Two rules, each a refusal by itself. */
function refuseOffOrigin(method: string, url: string, hostname: string): Refusal {
  // No private address; and no other origin at all.
  if (privateHost(hostname)) return refuseElsewhere(method, url, "not a public address");
  return refuseElsewhere(method, url, "another origin");
}

function isRead(method: string): boolean {
  return method === "GET" || method === "HEAD";
}

/**
 * The headers a request to `path` on the target's own origin is signed with,
 * when it is one the check signs: a read or the runtime's beacon under the
 * app's base, a query of the app's data, or the shell's bootstrap.
 */
function signedHeaders(
  target: LoadsTarget,
  method: string,
  path: string
): Record<string, string> | undefined {
  if (path.startsWith(target.basePath)) {
    const beacon = method === "POST" && path.endsWith("/__oxy/beacon");
    if (isRead(method) || beacon) return target.headers;
  }
  if (method === "POST" && target.dataPaths.includes(path)) return target.dataHeaders;
  if (isRead(method) && path === target.shellPath) return target.dataHeaders;
  return undefined;
}

/**
 * What `method requestUrl` gets. Judged after URL parsing, so a `..` that
 * climbs out of the base is judged where it lands.
 */
export function treat(target: LoadsTarget, method: string, requestUrl: string): Treatment {
  let url: URL;
  try {
    url = new URL(requestUrl);
  } catch {
    return refuseElsewhere(method, requestUrl, "not a URL");
  }
  // Before the origin: a `blob:` URL carries the origin of the page that made it.
  if (LOCAL_SCHEMES.includes(url.protocol)) return LOCAL;
  if (url.origin !== target.origin) return refuseOffOrigin(method, requestUrl, url.hostname);
  const path = url.pathname;
  const headers = signedHeaders(target, method, path);
  if (headers) return { kind: "sign", headers };
  // A preflight carries no body and changes nothing; the request it precedes is judged itself.
  if (isRead(method) || method === "OPTIONS") return PASS;
  return refuseWrite(method, path);
}

/**
 * What the next hop of a redirect gets, before the redirect is handed to the
 * browser. The browser follows a hop by itself, and whatever that hop answers
 * in turn, without the check being asked again. So only a READ is handed
 * over: a read stays a read on every hop after it, and a read on the app's
 * origin is one the check passes. A write that is redirected is not sent on.
 */
export function treatHop(target: LoadsTarget, method: string, hopUrl: string): Treatment {
  const treatment = treat(target, method, hopUrl);
  if (treatment.kind === "refuse" || treatment.kind === "local" || isRead(method)) return treatment;
  return refuseWrite(method, new URL(hopUrl).pathname);
}

/**
 * What a WebSocket to `socketUrl` gets: it is never opened. Off the app's
 * origin nothing is sent whatever it is; on it, a socket is a channel both
 * ways, and the checker reads.
 */
export function treatSocket(target: LoadsTarget, socketUrl: string): Refusal {
  let url: URL;
  try {
    url = new URL(socketUrl);
  } catch {
    return refuseElsewhere("WS", socketUrl, "not a URL");
  }
  // A socket's origin is the one its handshake goes to: `wss` is `https`.
  const origin = `${url.protocol === "wss:" ? "https" : "http"}://${url.host}`;
  if (origin !== target.origin) return refuseOffOrigin("WS", socketUrl, url.hostname);
  return refuseWrite("WS", url.pathname);
}

/**
 * Whether a request is the runtime's ready beacon (`<base>__oxy/beacon`
 * carrying `oxy-app-ready`).
 */
export function isReadyBeacon(requestUrl: string, postData: string | null): boolean {
  return requestUrl.includes("/__oxy/beacon") && (postData ?? "").includes("oxy-app-ready");
}

/** Loopback, private, link-local, shared (CGNAT), unspecified and broadcast addresses. */
const NOT_PUBLIC = new BlockList();
NOT_PUBLIC.addSubnet("127.0.0.0", 8);
NOT_PUBLIC.addSubnet("10.0.0.0", 8);
NOT_PUBLIC.addSubnet("172.16.0.0", 12);
NOT_PUBLIC.addSubnet("192.168.0.0", 16);
NOT_PUBLIC.addSubnet("169.254.0.0", 16);
NOT_PUBLIC.addSubnet("100.64.0.0", 10);
NOT_PUBLIC.addAddress("0.0.0.0");
NOT_PUBLIC.addAddress("255.255.255.255");
NOT_PUBLIC.addAddress("::1", "ipv6");
NOT_PUBLIC.addAddress("::", "ipv6");
NOT_PUBLIC.addSubnet("fc00::", 7, "ipv6");
NOT_PUBLIC.addSubnet("fe80::", 10, "ipv6");

/**
 * Whether a host is no public internet host: a private IP (an IPv4-mapped
 * IPv6 address is judged as its IPv4), or a name that only resolves inside a
 * network (`localhost`, a single label, `.local`, `.internal`, `.svc`, `.lan`).
 * By name and literal only: where a public-looking name resolves is not looked up.
 */
export function privateHost(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, "");
  if (host === "") return true;
  const family = isIP(host);
  if (family === 4) return NOT_PUBLIC.check(host);
  if (family === 6) return NOT_PUBLIC.check(host, "ipv6");
  const name = host.replace(/\.+$/, "").toLowerCase();
  return (
    !name.includes(".") ||
    name === "localhost" ||
    [".localhost", ".local", ".internal", ".svc", ".lan"].some((suffix) => name.endsWith(suffix)) ||
    name.includes(".svc.")
  );
}
