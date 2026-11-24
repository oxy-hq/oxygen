import type { CustomApp } from "@/types/apps";
import {
  type AppStatus,
  type HealthIndex,
  needsAttention,
  statusOf,
  statusRank
} from "./appStatus";

/**
 * What the column browser shows, as data — no React, no routing.
 *
 * The browser's three tiers are the registry's real shape: org → app → console page. The
 * first two are derived here from the app list the page already has, which is the whole
 * reason the browser costs no extra request. `CustomApp` carries `org_slug` and
 * `org_id` but no org *name*, and the two ways to get one are both per-row fetches
 * (`useAdminOrgLogo`) or a full paged walk of every org on the deployment including
 * those with no apps (`useAllAdminOrgs`). So the column is keyed and labelled by slug —
 * which is what `AppIdentity`, every URL and every `oxyc` command already call an app's
 * org, so it is the vocabulary an operator reads everywhere else on this surface.
 *
 * ## Ordering is alphabetical, and that is a deliberate break
 *
 * Everything else here orders worst-first (`byAttention`), and these columns do not.
 * Fleet health repolls every minute: a worst-first column re-sorts under an operator who
 * is arrowing through it, so the row you were about to press moves on a timer. A column
 * is navigated; a table is read. Nothing is lost by sorting it for navigation — every
 * row still carries its status, an org row carries how many of its apps need a person,
 * `FleetStrip` states the deployment-wide fact above everything, and the detail pane's
 * table is still worst-first. "What is worst?" is the table's question; "where is X?" is
 * the column's.
 */

/** Which apps the Apps column shows: every one, or one org's. */
export type BrowserScope = { kind: "all" } | { kind: "org"; orgSlug: string };

/**
 * How a row in the Apps tier is identified: `org/slug`, the same key
 * `useAdminAppRegistry` resolves the route with and the same pair that appears in a URL
 * and in an `oxyc` command. Not the uuid — a column's identity should be the thing an
 * operator can read back off the screen.
 */
export const appKey = (app: Pick<CustomApp, "org_slug" | "slug">): string =>
  `${app.org_slug}/${app.slug}`;

/** The worst thing true of a set of apps, for a row that stands for the set. */
export interface AppsRollup {
  appCount: number;
  /** How many need a person, by the same `needsAttention` the fleet ranks on. */
  attention: number;
  /**
   * The worst status among them, by `statusRank`, or `null` when the worst thing about
   * the set is that something in it is unmeasured — and when the set is empty.
   *
   * Both read as "unknown" on screen, which is the honest answer to both: `null` is
   * never a quiet all-clear, the same rule `statusOf` keeps.
   */
  worst: AppStatus | null;
}

export interface OrgRow extends AppsRollup {
  orgSlug: string;
}

/** The fact a row standing for `apps` states. */
export function rollup(apps: readonly CustomApp[], health: HealthIndex | undefined): AppsRollup {
  let attention = 0;
  let worst: AppStatus | null = null;
  let worstRank = Number.POSITIVE_INFINITY;
  for (const app of apps) {
    const status = statusOf(app, health);
    if (needsAttention(status)) attention += 1;
    // Ranked, not "the first verdict wins". `statusRank` puts the unknown *between* the
    // verdicts and the drafts, which is the whole subtlety here: a real `down` beats an
    // unmeasured app, and an unmeasured app beats a draft — a draft serves nobody, so it
    // is never the worst thing about a set that also holds something nobody measured.
    const rank = statusRank(status);
    if (rank < worstRank) {
      worstRank = rank;
      worst = status;
    }
  }
  return { appCount: apps.length, attention, worst };
}

/** One row per org that has an app, alphabetically by slug. */
export function orgRows(apps: readonly CustomApp[], health: HealthIndex | undefined): OrgRow[] {
  const byOrg = new Map<string, CustomApp[]>();
  for (const app of apps) {
    const bucket = byOrg.get(app.org_slug);
    if (bucket) bucket.push(app);
    else byOrg.set(app.org_slug, [app]);
  }
  return [...byOrg.entries()]
    .map(([orgSlug, group]) => ({ orgSlug, ...rollup(group, health) }))
    .sort((a, b) => a.orgSlug.localeCompare(b.orgSlug));
}

/**
 * The apps a scope names, alphabetically by name. The tie-break is org then slug, so two
 * apps sharing a name have a stable order and the one you arrowed to stays where it was.
 */
export function appsInScope(apps: readonly CustomApp[], scope: BrowserScope): CustomApp[] {
  const within = scope.kind === "org" ? apps.filter((a) => a.org_slug === scope.orgSlug) : apps;
  return [...within].sort(
    (a, b) =>
      a.name.localeCompare(b.name) ||
      a.org_slug.localeCompare(b.org_slug) ||
      a.slug.localeCompare(b.slug)
  );
}

/**
 * The scope the browser is in.
 *
 * An app decides it when there is one — the org column follows the app rather than
 * holding a stale selection beside it. `?org=` only means anything with no app selected,
 * and an org nothing is filed under is ignored rather than shown as an empty column: a
 * stale link lands on All apps instead of on a tier that cannot exist.
 */
export function scopeFrom(
  selectedApp: CustomApp | null,
  orgParam: string | null,
  orgs: readonly OrgRow[]
): BrowserScope {
  if (selectedApp) return { kind: "org", orgSlug: selectedApp.org_slug };
  if (orgParam && orgs.some((o) => o.orgSlug === orgParam)) {
    return { kind: "org", orgSlug: orgParam };
  }
  return { kind: "all" };
}

/** How long a type-ahead buffer stands before the next keystroke starts a new one. */
export const TYPE_AHEAD_MS = 700;

/**
 * Where a type-ahead buffer lands: the first row at or after the cursor whose label
 * starts with it, wrapping once, or `null` when nothing does.
 *
 * A single character searches from *after* the cursor, so pressing `s` repeatedly cycles
 * through everything beginning with it — the listbox convention, and the one that makes
 * two apps called "Store ops" reachable. Two or more search from the cursor itself, so
 * typing `s`, `t`, `o` refines onto one row instead of skipping past it.
 */
export function typeAheadTarget(
  labels: readonly string[],
  buffer: string,
  cursor: number
): number | null {
  const needle = buffer.toLowerCase();
  if (!needle || labels.length === 0) return null;
  const from = cursor + (needle.length > 1 ? 0 : 1);
  for (let step = 0; step < labels.length; step += 1) {
    const index = (from + step + labels.length) % labels.length;
    if (labels[index].toLowerCase().startsWith(needle)) return index;
  }
  return null;
}
