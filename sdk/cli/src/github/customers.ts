/**
 * Who the customers are — derived from GitHub, never read from a file.
 *
 * Membership is the repo topic `oxy-customer` in `$OXYC_ORG` and nothing else,
 * so creating or tagging the repo IS the registration. There is no suffix
 * rule: `-oxy` and the legacy `-context` are just names, and `pokehouse-oxy`
 * and `pokehouse-context` may both exist.
 *
 * Every refusal below is `customer-tooling/lib/customers.sh`'s, ported with
 * its reasoning intact. See `gh.ts` for the safety rule they all serve.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";
import { ensureDir, oxycCacheDir, slugifyForPath } from "../util/paths.js";
import { ghJson, ghRaw, refuseIfAtLimit } from "./gh.js";

export interface Customer {
  /** The repo name, which is the customer's identity. */
  name: string;
  /** The repo description, used as a display name. May be empty. */
  description: string;
}

interface CustomerCache {
  org: string;
  fetchedAt: number;
  customers: Customer[];
}

/** The org customer repos live in. */
export function customersOrg(): string {
  return process.env.OXYC_ORG ?? "oxy-hq";
}

/** The topic that decides membership. One place, so read and write agree. */
export function customersTopic(): string {
  return process.env.OXYC_CUSTOMER_TOPIC ?? "oxy-customer";
}

/**
 * The ceiling `gh` is asked for. A listing that comes back exactly AT it is
 * refused rather than served — see `refuseIfAtLimit`.
 */
function listLimit(): number {
  const raw = Number(process.env.OXYC_LIST_LIMIT ?? "1000");
  return Number.isFinite(raw) && raw > 0 ? raw : 1000;
}

/**
 * An hour. Long enough that a session's worth of commands costs one `gh` call,
 * short enough that a customer created this morning shows up before lunch.
 */
function cacheTtlMs(): number {
  const raw = Number(process.env.OXYC_CACHE_TTL ?? "3600");
  return (Number.isFinite(raw) && raw >= 0 ? raw : 3600) * 1000;
}

/**
 * The cache file, keyed by ORG in its NAME and again in its CONTENTS.
 *
 * Switching `$OXYC_ORG` must not serve another org's customers, and a single
 * shared file would do exactly that until the TTL expired. A filename cannot
 * carry every character an org name might, so the sanitised name is the
 * convenience and the stored field is the identity.
 */
function cacheFile(org: string): string {
  // 0700/0600 like the response and catalog caches. This file is the customer
  // list of the business, read out of a PRIVATE GitHub org — leaving it
  // world-readable would publish to every local user exactly what `gh` had to
  // authenticate to see.
  return join(ensureDir(join(oxycCacheDir(), "customers"), 0o700), `${slugifyForPath(org)}.json`);
}

function readCache(org: string): CustomerCache | undefined {
  try {
    const cached = JSON.parse(readFileSync(cacheFile(org), "utf8")) as CustomerCache;
    if (cached.org !== org || !Array.isArray(cached.customers)) return undefined;
    return cached;
  } catch {
    return undefined;
  }
}

function writeCache(org: string, customers: Customer[]): void {
  try {
    writeFileSync(cacheFile(org), JSON.stringify({ org, fetchedAt: Date.now(), customers }), {
      mode: 0o600
    });
  } catch {
    log.warn("could not write the customer cache; every command will ask GitHub");
  }
}

/** Drop the cached listing, so the next read asks GitHub. */
export function invalidateCache(org = customersOrg()): void {
  try {
    writeFileSync(cacheFile(org), JSON.stringify({ org, fetchedAt: 0, customers: [] }), {
      mode: 0o600
    });
  } catch {
    // Nothing to do: a cache that cannot be invalidated expires on its own.
  }
}

/**
 * Confirm `gh` can actually see private repos before asking it to list them.
 *
 * THE SINGLE MOST DANGEROUS SHAPE this module can be handed: customer repos
 * are private, so an unauthenticated `gh repo list` succeeds and returns a
 * plausible EMPTY ARRAY. Every refusal downstream is looking for a failure,
 * and this one does not look like a failure — so it is checked up front, as a
 * precondition, rather than inferred from a result that will never say it.
 */
function requireGhAuth(org: string): void {
  const status = ghRaw(["auth", "status"]);
  if (status.status !== 0) {
    throw new CliError("gh is installed but not authenticated", {
      code: ExitCode.AUTH,
      hint: "gh auth login",
      detail:
        `Customer repos in ${org} are private, so an unauthenticated listing comes back\n` +
        `EMPTY — which is not "${org} has no customers".`
    });
  }
}

/** Ask GitHub. Every failure mode is loud; none of them returns an empty list. */
function fetchCustomers(org: string): Customer[] {
  requireGhAuth(org);
  const limit = listLimit();

  const repos = ghJson<Array<{ name?: unknown; description?: unknown }>>(
    [
      "repo",
      "list",
      org,
      "--topic",
      customersTopic(),
      "--limit",
      String(limit),
      "--json",
      "name,description"
    ],
    `the customer list for ${org}`
  );

  // The shape is validated, not assumed. A `gh` that exits 0 having printed
  // something which is not an array of named repos has told us nothing, and
  // nothing must not be allowed to read as an empty list.
  if (!Array.isArray(repos)) {
    throw new CliError(`gh returned something that is not a list of repos for ${org}`, {
      code: ExitCode.UNAVAILABLE,
      hint: `this is not "${org} has no customers"`
    });
  }
  for (const repo of repos) {
    if (typeof repo.name !== "string" || !repo.name) {
      throw new CliError("a repo came back with no name, so the listing is unusable", {
        code: ExitCode.UNAVAILABLE
      });
    }
  }

  refuseIfAtLimit(repos.length, limit, `the customer listing for ${org}`);

  return repos
    .map((repo) => ({
      name: repo.name as string,
      // Tabs, CRs and newlines squeezed out so a description holds the line
      // format for any value GitHub will accept.
      description: (typeof repo.description === "string" ? repo.description : "").replace(
        /[\t\r\n]+/g,
        " "
      )
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * The customers, from cache when it is fresh.
 *
 * On a live failure with a cache in hand the cache is served STALE AND SAID
 * SO, because degrading to stale-but-correct is the point of having one.
 * Degrading to EMPTY is the failure it exists to prevent.
 */
export function listCustomers(opts: { refresh?: boolean; org?: string } = {}): Customer[] {
  const org = opts.org ?? customersOrg();
  const cached = readCache(org);

  if (!opts.refresh && cached && Date.now() - cached.fetchedAt < cacheTtlMs()) {
    return cached.customers;
  }

  try {
    const customers = fetchCustomers(org);
    writeCache(org, customers);
    return customers;
  } catch (cause) {
    if (cached?.customers.length) {
      const ageMinutes = Math.round((Date.now() - cached.fetchedAt) / 60_000);
      log.warn(`serving a STALE customer list (${ageMinutes}m old): ${(cause as Error).message}`);
      return cached.customers;
    }
    throw cause;
  }
}

/**
 * Resolve a name to a customer.
 *
 * The "unknown customer" message points at `--refresh` deliberately: a
 * customer a teammate registered minutes ago is invisible to every command
 * until the hour-long cache expires, and that is by far the likeliest reason a
 * real name does not resolve.
 */
export function resolveCustomer(name: string, opts: { refresh?: boolean } = {}): Customer {
  const customers = listCustomers(opts);
  const exact = customers.find((c) => c.name === name);
  if (exact) return exact;

  // A bare `pokehouse` should find `pokehouse-oxy`: the suffix is a naming
  // habit, not part of the identity, and making people type it would be
  // making them remember which of two habits a given repo used.
  const prefixed = customers.filter((c) => c.name.startsWith(`${name}-`));
  if (prefixed.length === 1) return prefixed[0] as Customer;
  if (prefixed.length > 1) {
    throw new CliError(`"${name}" matches ${prefixed.length} customers`, {
      code: ExitCode.USAGE,
      detail: prefixed.map((c) => `  ${c.name}`).join("\n"),
      hint: "name one of them exactly"
    });
  }

  throw new CliError(`unknown customer "${name}"`, {
    code: ExitCode.NOT_FOUND,
    hint: "oxyc list --refresh   — a customer registered in the last hour may not be cached yet"
  });
}

/** `<org>/<name>` — the slug that identifies a customer everywhere. */
export function customerSlug(customer: Customer, org = customersOrg()): string {
  return `${org}/${customer.name}`;
}

/** The display name: the repo description, falling back to the repo name. */
export function displayName(customer: Customer): string {
  return customer.description.trim() || customer.name;
}
