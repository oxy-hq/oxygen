import type { SandboxAgentLimits, SandboxApp, TokenOptions } from "@/types/apiToken";

/**
 * A sandbox agent token (`oxy_sbx_…`), as the two places that mint one need it: the create dialog
 * under Account → Personal access tokens, and the `/cli-auth` approval `oxyc tokens create
 * --sandbox-agent` opens. Both read the same limits, name an app the same way and say the same
 * thing about what the token can do.
 */

/** The contract's numbers, for a `token-options` answer that names no limits of its own. */
export const DEFAULT_SANDBOX_LIMITS: SandboxAgentLimits = {
  default_hours: 8,
  max_hours: 168,
  max_apps: 5
};

export const MIN_SANDBOX_HOURS = 1;

export const sandboxLimits = (
  options: Pick<TokenOptions, "sandbox_agent"> | undefined
): SandboxAgentLimits => options?.sandbox_agent ?? DEFAULT_SANDBOX_LIMITS;

/** The apps the caller may mint for. Empty means the type is not offered at all. */
export const sandboxApps = (
  options: Pick<TokenOptions, "sandbox_apps"> | undefined
): SandboxApp[] => options?.sandbox_apps ?? [];

/** What a holder may do, one act each, worded to follow "can". */
const canDo = (apps: string): string[] => [
  `create up to three dev sandboxes of ${apps}`,
  "publish into them",
  "call their functions",
  "run their checks",
  "read their logs",
  "set their secrets"
];

/** What a holder may not do, one act each, worded to follow "can't". */
const CANNOT_DO = [
  "reach production or staging",
  "promote a build",
  "read a secret's value",
  "open the admin console",
  "touch any other app"
];

const capitalized = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

/** "a, b and c", or with "or" for what is refused. */
const inSeries = (acts: string[], last: "and" | "or"): string =>
  `${acts.slice(0, -1).join(", ")} ${last} ${acts[acts.length - 1]}`;

/**
 * What the token can and cannot do, one line per act, for an approval that is scanned and not
 * read. `apps` names them from where the reader stands: "these apps" on a page that lists them.
 */
export const sandboxAgentPowers = (apps: string): { can: string[]; cannot: string[] } => ({
  can: canDo(apps).map(capitalized),
  cannot: CANNOT_DO.map(capitalized)
});

/**
 * The same two lists as two sentences, where height is scarce. Each is set beside its own label,
 * "Can" and "Cannot", so it starts with the act. `apps` is "the apps you pick" in the create
 * dialog.
 */
export const sandboxAgentSummary = (apps: string): { can: string; cannot: string } => ({
  can: `${capitalized(inSeries(canDo(apps), "and"))}.`,
  cannot: `${capitalized(inSeries(CANNOT_DO, "or"))}.`
});

const plural = (count: number, noun: string): string => `${count} ${noun}${count === 1 ? "" : "s"}`;

/** "1 hour", "24 hours", "3 days": whole days from two up, where a day count reads faster. */
export const lifetimeLabel = (hours: number): string =>
  hours >= 48 && hours % 24 === 0 ? plural(hours / 24, "day") : plural(hours, "hour");

const LIFETIME_PRESETS = [1, 8, 24, 72, 168] as const;

/**
 * The lifetimes on offer: the presets the server's maximum allows, the maximum itself when it
 * falls between two of them, and the server's default when it is none of them.
 */
export const lifetimePresets = (limits: SandboxAgentLimits): number[] => {
  const hours = new Set<number>(LIFETIME_PRESETS.filter((preset) => preset <= limits.max_hours));
  hours.add(limits.max_hours);
  if (limits.default_hours <= limits.max_hours) hours.add(limits.default_hours);
  return [...hours].filter((value) => value >= MIN_SANDBOX_HOURS).sort((a, b) => a - b);
};

const maxLifetime = (limits: SandboxAgentLimits): string => {
  const inHours = plural(limits.max_hours, "hour");
  const label = lifetimeLabel(limits.max_hours);
  return label === inHours ? inHours : `${inHours} (${label})`;
};

/** Why a lifetime can't be used, or `null`. The server answers 400 to the same thing. */
export const hoursProblem = (hours: number, limits: SandboxAgentLimits): string | null => {
  if (!Number.isInteger(hours)) return "Enter a whole number of hours.";
  if (hours < MIN_SANDBOX_HOURS) return "A sandbox agent token lasts at least 1 hour.";
  if (hours > limits.max_hours) {
    return `A sandbox agent token lasts at most ${maxLifetime(limits)}.`;
  }
  return null;
};

/** Why this many apps can't be named, or `null`. */
export const appCountProblem = (count: number, limits: SandboxAgentLimits): string | null => {
  if (count === 0) return "Pick at least one app.";
  if (count > limits.max_apps) {
    return `A sandbox agent token covers at most ${plural(limits.max_apps, "app")}.`;
  }
  return null;
};

const HOUR_MS = 60 * 60 * 1000;

/** When a token minted now for this many hours stops working. */
export const sandboxExpiry = (hours: number, now = new Date()): Date =>
  new Date(now.getTime() + hours * HOUR_MS);

/** `acme/store-ops`: how oxyc names an app, and what the picker shows beside its name. */
export const sandboxAppRef = (app: Pick<SandboxApp, "org_slug" | "slug">): string =>
  `${app.org_slug}/${app.slug}`;

/** The app an `<org>/<app>` reference names, if the caller may mint for it. Case is ignored. */
export const findSandboxApp = (
  apps: readonly SandboxApp[],
  ref: string
): SandboxApp | undefined => {
  const wanted = ref.trim().toLowerCase();
  return apps.find((app) => sandboxAppRef(app).toLowerCase() === wanted);
};

export interface SandboxAppGroup {
  orgId: string;
  orgName: string;
  apps: SandboxApp[];
}

const matches = (app: SandboxApp, query: string): boolean =>
  [app.name, app.slug, app.org_name, app.org_slug, sandboxAppRef(app)].some((field) =>
    field.toLowerCase().includes(query)
  );

/**
 * The picker's list: apps under their org, both in name order, narrowed by what was typed. A
 * query matches an app's name or slug, its org's, or the `<org>/<app>` reference as a whole.
 */
export const groupSandboxApps = (apps: readonly SandboxApp[], query = ""): SandboxAppGroup[] => {
  const needle = query.trim().toLowerCase();
  const groups = new Map<string, SandboxAppGroup>();
  for (const app of apps) {
    if (needle && !matches(app, needle)) continue;
    const group = groups.get(app.org_id) ?? { orgId: app.org_id, orgName: app.org_name, apps: [] };
    group.apps.push(app);
    groups.set(app.org_id, group);
  }
  return [...groups.values()]
    .map((group) => ({ ...group, apps: group.apps.sort((a, b) => a.name.localeCompare(b.name)) }))
    .sort((a, b) => a.orgName.localeCompare(b.orgName));
};
