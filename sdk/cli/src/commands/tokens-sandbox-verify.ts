/**
 * Is what the exchange returned THE TOKEN THAT WAS ASKED FOR? Decided before a
 * byte of it is printed, by `oxyc tokens create --sandbox-agent`.
 *
 * A web app or server that predates the kind ignores the four parameters. Its
 * page then shows the ordinary "Authorize oxyc" screen, and its exchange
 * returns a ninety-day login token with the approver's whole reach. An agent
 * that asked for a few hours on one app must never be handed that — or any
 * token that is wider or longer-lived than what it asked for.
 *
 * Accepted only when ALL of these hold:
 *
 *   1. the secret is an `oxy_sbx_`;
 *   2. it is described as `kind: "sandbox_agent"`, and not as all-access;
 *   3. it reaches exactly the apps that were named;
 *   4. it expires, and no later than the lifetime asked for.
 *
 * THE RULE THAT MATTERS MOST: WHAT CANNOT BE READ IS REFUSED. An app entry
 * with no slug, an expiry that does not parse, a field that is simply absent —
 * each is "not confirmable", never "nothing to object to". A check that skips
 * what it cannot read accepts a token for the apps asked for plus one more it
 * could not parse.
 *
 * Apps and expiry are read off the exchange's own description. Whichever of
 * the two it does not carry is read back from the deployment with the new
 * token (`GET /api/auth/token`), once.
 */

import { parseJson } from "../api/request.js";
import { describeSandboxToken } from "../apps/sandbox-token.js";
import type { Minted } from "../auth/login.js";
import { isSandboxAgentToken, isWellFormedSandboxToken } from "../auth/token-kind.js";

/** What is wrong with what the deployment handed back, in two parts. */
export interface Mismatch {
  /** The headline: what happened. */
  what: string;
  /** What it returned, against what was asked for. */
  why: string;
}

export interface Asked {
  /** `<org>/<app>`, as named. */
  apps: string[];
  hours: number;
}

/** How far a server's clock, and the round trip, may put the expiry past the ask. */
const CLOCK_SLACK_MS = 5 * 60_000;
const HOUR_MS = 3_600_000;

type Described = Record<string, unknown>;

/** The headline for a deployment that predates the kind altogether. */
export const unsupported = (target: string) =>
  `${target} does not support sandbox agent tokens yet`;
/** The two headlines every mint shares: `tokens create --agent` raises them too. */
export const notAsked = (target: string) =>
  `${target} minted a token that is not the one asked for`;
export const unconfirmed = (target: string) =>
  `${target} minted a token that could not be confirmed`;

/** One app as a comparable key. A pair, so no slug can forge the separator. */
const key = (org: string, slug: string) => JSON.stringify([org.toLowerCase(), slug.toLowerCase()]);

/**
 * The apps a description lists, as `org/slug` — or why they cannot be trusted.
 * EVERY entry must be an object with a non-empty string `org_slug` and `slug`.
 * One that is not makes the whole list unreadable: dropping it would compare
 * the rest and call that a match.
 */
function readApps(raw: unknown): { apps: [string, string][] } | { unreadable: string } {
  if (!Array.isArray(raw)) return { unreadable: "it lists no apps" };
  const apps: [string, string][] = [];
  for (const entry of raw) {
    const app = entry as { org_slug?: unknown; slug?: unknown } | null;
    const org = typeof app?.org_slug === "string" ? app.org_slug.trim() : "";
    const slug = typeof app?.slug === "string" ? app.slug.trim() : "";
    if (typeof app !== "object" || !org || !slug) {
      return { unreadable: `one of the ${raw.length} apps it lists has no org_slug and slug` };
    }
    apps.push([org, slug]);
  }
  return { apps };
}

/** Why the apps are not exactly the ones asked for, if they are not. */
function appsMismatch(target: string, ask: Asked, raw: unknown): Mismatch | undefined {
  const read = readApps(raw);
  if ("unreadable" in read) {
    return {
      what: unconfirmed(target),
      why: `which apps it reaches cannot be read: ${read.unreadable}`
    };
  }
  const got = new Set(read.apps.map(([org, slug]) => key(org, slug)));
  const asked = new Set(
    ask.apps.map((app) => {
      const [org = "", slug = ""] = app.split("/");
      return key(org, slug);
    })
  );
  // The COUNT of entries, not of distinct keys: a duplicate is one more grant.
  if (read.apps.length === asked.size && [...asked].every((app) => got.has(app))) return undefined;
  const reached = read.apps.map(([org, slug]) => `${org}/${slug}`).join(", ") || "no app";
  return {
    what: notAsked(target),
    why: `it reaches ${reached}, and ${ask.apps.join(", ")} was asked for`
  };
}

/**
 * Why the lifetime is not the bounded one asked for, if it is not. The same
 * rule, and the same clock slack, for every token this CLI mints.
 */
export function lifetimeMismatch(
  target: string,
  ask: Pick<Asked, "hours">,
  raw: unknown,
  now: number
): Mismatch | undefined {
  const wanted = `${ask.hours} h was asked for`;
  if (raw === null || raw === undefined || raw === "") {
    return { what: notAsked(target), why: `it has no expiry, and ${wanted}` };
  }
  const at = typeof raw === "string" ? Date.parse(raw) : Number.NaN;
  if (typeof raw !== "string" || Number.isNaN(at)) {
    return {
      what: unconfirmed(target),
      why: `its expiry ${JSON.stringify(raw)} cannot be read as a time, and ${wanted}`
    };
  }
  if (at > now + ask.hours * HOUR_MS + CLOCK_SLACK_MS) {
    const hours = Math.round(((at - now) / HOUR_MS) * 10) / 10;
    return {
      what: notAsked(target),
      why: `it expires ${raw} — ${hours} h from now — and ${wanted}`
    };
  }
  return undefined;
}

/** The deployment's own description of the new token, when the exchange left a field out. */
async function readBack(target: string, secret: string): Promise<Described | Mismatch> {
  try {
    const { body } = await describeSandboxToken(target, secret, { fresh: true });
    const parsed = parseJson(body);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as Described;
    return { what: unconfirmed(target), why: "its description could not be read back" };
  } catch (cause) {
    return {
      what: unconfirmed(target),
      why: `its description could not be read back: ${(cause as Error).message}`
    };
  }
}

function isMismatch(value: Described | Mismatch): value is Mismatch {
  return typeof value.what === "string" && typeof value.why === "string";
}

/**
 * `undefined` when the minted credential is exactly what was asked for;
 * otherwise what is wrong with it. `now` is a parameter for the tests.
 */
export async function mismatch(
  target: string,
  ask: Asked,
  minted: Minted,
  now: number = Date.now()
): Promise<Mismatch | undefined> {
  if (!isSandboxAgentToken(minted.token)) {
    return {
      what: unsupported(target),
      why: "its /cli-auth page ran an ordinary `oxyc login` and returned a login token"
    };
  }
  // The prefix chose the path; the whole shape is what may be printed. The
  // secret ends up in a line a shell evaluates, so it is letters and digits or
  // it is refused, and it is never quoted back in the message.
  if (!isWellFormedSandboxToken(minted.token)) {
    return {
      what: notAsked(target),
      why: "the secret it returned is not a whole sandbox agent token (`oxy_sbx_` and 36 letters or digits)"
    };
  }
  const exchanged: Described = minted.described ?? {};
  if (exchanged.kind !== "sandbox_agent") {
    return {
      what: unsupported(target),
      why: `its exchange described the token as kind ${JSON.stringify(exchanged.kind ?? null)}, not "sandbox_agent"`
    };
  }

  // ABSENT is read back; PRESENT is believed, `null` included — a description
  // that says "no expiry" is not asked a second time in the hope of a better one.
  let confirmed: Described = {};
  if (exchanged.apps === undefined || exchanged.expires_at === undefined) {
    const read = await readBack(target, minted.token);
    if (isMismatch(read)) return read;
    confirmed = read;
  }
  if (exchanged.all_access === true || confirmed.all_access === true) {
    return { what: notAsked(target), why: "it is described as all-access" };
  }

  const apps = exchanged.apps !== undefined ? exchanged.apps : confirmed.apps;
  const expiresAt =
    exchanged.expires_at !== undefined ? exchanged.expires_at : confirmed.expires_at;
  return appsMismatch(target, ask, apps) ?? lifetimeMismatch(target, ask, expiresAt, now);
}
