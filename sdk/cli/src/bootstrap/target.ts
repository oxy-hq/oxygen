/**
 * What a standup is ABOUT, parsed and validated before anything is read.
 *
 * EVERY USAGE ERROR BEFORE A REQUEST, as `oxyc oltp provision` has it: a typo
 * in a writer spec should not first cost an org lookup, a login and — here —
 * a billable provision at a provider.
 */

import { isValidSlug } from "@oxy-hq/sdk/manifest";
import { customersOrg } from "../github/customers.js";
import { usageError } from "../util/errors.js";
import { isPhaseId, PHASE_IDS, type PhaseId } from "./phases.js";

export interface BootstrapTarget {
  /** The org slug, and by default the customer repo's name and the app's slug. */
  slug: string;
  /** The org's display name. */
  name: string;
  /** The person onboarded as Org Owner. Absent when only verifying. */
  ownerEmail?: string;
  /** `<owner>/<repo>` for the workspace repository. */
  repo: string;
  /** The first app's slug. */
  app: string;
  /** OLTP writers to ensure, on the wire (`app:<slug>` / `pipeline:<source>`). */
  writers: string[];
  /** The customer has frontline crews, so the frontline phase applies. */
  crews: boolean;
}

export interface BootstrapFlags {
  name?: string;
  owner?: string;
  repo?: string;
  app?: string;
  writer?: string[];
  crews?: boolean;
  phase?: string[];
  yes?: boolean;
  json?: boolean;
  dryRun?: boolean;
}

/** A pragmatic shape check — the server is the authority on deliverability. */
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const REPO = /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/;

/**
 * The target, from the positional slug and the flags.
 *
 * `--owner` is required only where a write needs it, so parsing takes
 * `requireOwner` rather than deciding for every verb: `verify` must work on an
 * org whose owner nobody remembers.
 */
export function parseTarget(
  slug: string,
  flags: BootstrapFlags,
  opts: { requireOwner: boolean }
): BootstrapTarget {
  const trimmed = slug.trim();
  if (!isValidSlug(trimmed)) {
    throw usageError(
      `${JSON.stringify(slug)} is not an organization slug`,
      "lowercase letters and digits in words joined by single hyphens — e.g. poke-house. It is also the default name of the customer repository and of the first app"
    );
  }

  const owner = flags.owner?.trim();
  if (owner !== undefined && !EMAIL.test(owner)) {
    throw usageError(`--owner ${JSON.stringify(flags.owner)} is not an email address`);
  }
  if (owner === undefined && opts.requireOwner) {
    throw usageError(
      "--owner <email> names the person onboarded as the org's Owner",
      "a known account is added as Owner immediately; anyone else is sent an Owner invitation that expires in 7 days"
    );
  }

  const repo = flags.repo?.trim() ?? `${customersOrg()}/${trimmed}`;
  if (!REPO.test(repo)) {
    throw usageError(`--repo ${JSON.stringify(repo)} is not <owner>/<repo>`);
  }

  const app = flags.app?.trim() ?? trimmed;
  if (!isValidSlug(app)) {
    throw usageError(`--app ${JSON.stringify(flags.app)} is not an app slug`);
  }

  return {
    slug: trimmed,
    name: flags.name?.trim() || trimmed,
    ...(owner === undefined ? {} : { ownerEmail: owner }),
    repo,
    app,
    writers: parseWriters(flags.writer ?? [], app),
    crews: flags.crews === true
  };
}

/**
 * The writers to ensure on the org's OLTP store.
 *
 * Defaults to the first app's own writer, because an app that declares
 * `migrations` and has no schema fails at its first query — so "no writers"
 * is almost never what the caller meant, and the default is the one they
 * would have typed.
 */
export function parseWriters(typed: string[], appSlug: string): string[] {
  if (typed.length === 0) return [`app:${appSlug}`];
  const out: string[] = [];
  for (const raw of typed) {
    const spec = raw.trim();
    const [kind, rest, ...extra] = spec.split(":");
    if (extra.length > 0 || !rest || (kind !== "app" && kind !== "pipeline")) {
      throw usageError(
        `--writer ${JSON.stringify(raw)} is not app:<slug> or pipeline:<source>`,
        "app:<slug> for a custom app's ctx.oltp store, pipeline:<source> for an Airway pipeline"
      );
    }
    if (!out.includes(spec)) out.push(spec);
  }
  return out;
}

/** `--phase` values, checked against the table before anything is read. */
export function parsePhases(typed: string[]): PhaseId[] {
  const out: PhaseId[] = [];
  for (const raw of typed) {
    const id = raw.trim();
    if (!isPhaseId(id)) {
      throw usageError(
        `--phase ${JSON.stringify(raw)} is not a phase`,
        `one of: ${PHASE_IDS.join(", ")}`
      );
    }
    if (!out.includes(id)) out.push(id);
  }
  return out;
}
