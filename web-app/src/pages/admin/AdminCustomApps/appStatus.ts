import type { AppHealth, CustomApp } from "@/types/apps";

/**
 * What one custom app's state *is*, for every surface that has to say it.
 *
 * Salvaged verbatim from `AppsTable/useAppsTable.ts` when the registry table was
 * deleted. The table's view machinery — gallery vs list, group-by, the status chips
 * that all resolved to the same set — went with it; this did not, because it is the
 * only part that encoded domain knowledge rather than a layout preference. The
 * switcher, the console header and the storage audit all need to rank and name an
 * app's state, and three copies of that judgement would drift.
 */

/**
 * One state, not two. Publish state (live/draft) and fleet health answer the same
 * question — is this app OK? — so a published app reports its health verdict and an
 * unpublished one reports `draft`. A draft serves nobody, so it has no health to report.
 */
export type AppStatus = AppHealth | "draft";

/**
 * Health rows by app id. Typed to the one field the model reads, so a test can pass
 * `{ health }` while a page passes whole `AppHealthRow`s — the rows carry a reason and
 * a request count, this does not need them.
 */
export type HealthIndex = ReadonlyMap<string, { health: AppHealth }>;

/**
 * The verdicts that need a person — the same three the backend's
 * `AppHealth::needs_attention` names, so the two cannot disagree.
 */
export const ATTENTION: ReadonlySet<AppStatus> = new Set<AppStatus>([
  "down",
  "degraded",
  "not_measured"
]);

/**
 * Worst first. An app whose health is not known yet ranks after every verdict and
 * before drafts: it may be fine, and a draft serves nobody.
 */
const STATUS_RANK: Record<AppStatus, number> = {
  down: 0,
  degraded: 1,
  not_measured: 2,
  quiet: 3,
  operational: 4,
  draft: 6
};
const UNKNOWN_RANK = 5;

/**
 * An app's status, or `null` when its health is not known.
 *
 * `null` covers two cases and deliberately does not guess between them: health is
 * still loading, or the app is past the fleet endpoint's page cap. Reporting either as
 * `not_measured` would claim capture is off for an app nobody asked about — the same
 * class of lie the fleet fact exists to stop telling.
 */
export function statusOf(app: CustomApp, health: HealthIndex | undefined): AppStatus | null {
  if (!app.published_at) return "draft";
  return health?.get(app.id)?.health ?? null;
}

/** Sort key for "worst first". Unknown sits between the verdicts and drafts. */
export const statusRank = (s: AppStatus | null): number =>
  s === null ? UNKNOWN_RANK : STATUS_RANK[s];

/** Does this state want a person to look at it? Unknown does not — nobody measured it. */
export const needsAttention = (s: AppStatus | null): boolean => s !== null && ATTENTION.has(s);

/**
 * How a state reads on screen. `not_measured` says "not measured" and nothing more —
 * *why* nothing is measured is a deployment-wide fact stated once by `FleetStrip`, not
 * a sentence repeated once per app, which is what it used to be.
 */
export const STATUS_LABEL: Record<AppStatus, string> = {
  down: "Down",
  degraded: "Degraded",
  not_measured: "Not measured",
  quiet: "Quiet",
  operational: "Operational",
  draft: "Draft"
};

/**
 * Worst-first ordering over a fleet. This is what lets `/admin/apps` open on the app
 * that most needs someone rather than on whichever one sorts first alphabetically.
 * Ties break on name then org so the choice is stable across reloads — an operator who
 * refreshes should land in the same place.
 */
export function byAttention(
  apps: readonly CustomApp[],
  health: HealthIndex | undefined
): CustomApp[] {
  return [...apps].sort(
    (a, b) =>
      statusRank(statusOf(a, health)) - statusRank(statusOf(b, health)) ||
      a.name.localeCompare(b.name) ||
      a.org_slug.localeCompare(b.org_slug)
  );
}

/**
 * Does this app match what the operator typed? Name, org and slug all match, because
 * name alone does not identify an app — `oxy-starter` exists under two different orgs,
 * so someone typing "acme" must be able to narrow to one of them.
 */
export function matchesQuery(app: CustomApp, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    app.name.toLowerCase().includes(q) ||
    app.slug.toLowerCase().includes(q) ||
    app.org_slug.toLowerCase().includes(q) ||
    `${app.org_slug}/${app.slug}`.includes(q)
  );
}
