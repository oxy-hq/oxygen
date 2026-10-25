import type { AdminTone } from "@/pages/admin/components/adminTone";
import type { AppAvailabilityReading, AppHealth, CustomApp } from "@/types/apps";

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
 * Three sources, freshest first. The polled fleet-health row (`health`) wins when it has
 * one: it refreshes every minute, while a list row is read once and a console left open on
 * a second monitor would keep showing yesterday's `operational`. The row's own server
 * verdict (`app.verdict`, the same evaluator, from the list response) answers when the
 * fleet has not — still loading, past its page cap, or a surface that never polls it.
 * An unpublished app is a `draft` before either is read.
 *
 * `null` covers two cases and deliberately does not guess between them: health is
 * still loading, or the app is past the fleet endpoint's page cap. Reporting either as
 * `not_measured` would claim capture is off for an app nobody asked about — the same
 * class of lie the fleet fact exists to stop telling.
 */
export function statusOf(app: CustomApp, health: HealthIndex | undefined): AppStatus | null {
  if (!app.published_at) return "draft";
  return health?.get(app.id)?.health ?? app.verdict ?? null;
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
 * A status's tone. `draft` and `quiet` are deliberately `muted` rather than `ok`: neither
 * is a verdict that the app is working, and a green pill beside an app nobody has
 * measured or visited is the kind of false comfort this whole surface is being rebuilt
 * to stop giving.
 */
export const STATUS_TONE: Record<AppStatus, AdminTone> = {
  down: "danger",
  degraded: "warn",
  not_measured: "warn",
  quiet: "muted",
  operational: "ok",
  draft: "muted"
};

export const statusTone = (s: AppStatus): AdminTone => STATUS_TONE[s];

// ── One verdict, four sources ───────────────────────────────────────────────

/**
 * The one answer to "is this app OK?", for every surface that states it: the console
 * header, the lifecycle card, the fleet row, the partner rows and the admin home's
 * findings.
 *
 * Four sources feed it and they do not share a vocabulary — which is why, before this,
 * the product had five: `AppHealth` (down / degraded / not_measured / quiet /
 * operational), `AvailabilityVerdict` (no_opinion / healthy / burning), the partner's
 * live/draft, `AppStatus`'s `draft`, and the admin home's healthy/degraded/down/stale.
 * None of the wire types change here; the mapping below is purely client-side, and
 * `status` keeps the `AppStatus` vocabulary so `byAttention` / `statusRank` keep
 * ordering the fleet on it.
 *
 * ## The mapping
 *
 * | published | fleet health | availability     | verdict                        |
 * | --------- | ------------ | ---------------- | ------------------------------ |
 * | no        | (ignored)    | (ignored)        | `draft` · muted · "Draft"      |
 * | yes       | down         | any              | `down` · danger · "Down"       |
 * | yes       | degraded     | any              | `degraded` · warn · "Degraded" |
 * | yes       | not_measured | burning (page)   | `down` · danger · "Burning"    |
 * | yes       | not_measured | burning (ticket) | `degraded` · warn · "Degraded" |
 * | yes       | not_measured | else / unknown   | `not_measured` · warn          |
 * | yes       | quiet        | burning          | as burning above               |
 * | yes       | quiet        | else / unknown   | `quiet` · muted · "Quiet"      |
 * | yes       | operational  | burning          | as burning above               |
 * | yes       | operational  | else / unknown   | `operational` · ok             |
 * | yes       | unknown      | burning (page)   | `down` · danger · "Burning"    |
 * | yes       | unknown      | burning (ticket) | `degraded` · warn · "Degraded" |
 * | yes       | unknown      | healthy          | `operational` · ok · "Serving" |
 * | yes       | unknown      | no_opinion       | `quiet` · muted · "No data"    |
 * | yes       | unknown      | unknown          | `null` · muted · "Published"   |
 *
 * "fleet health" in the table is `input.health` (the polled fleet-health row) when present,
 * else `input.verdict` (the server's verdict, carried on the list rows). The columns are
 * the client mapping and stay as the fallback: a surface whose response carries no
 * `verdict` reads exactly the table above.
 *
 * Three rules make the table, and each is one of this surface's standing commitments:
 *
 * - **Worse wins.** Availability can only *escalate* a fleet verdict (`burning` over
 *   anything), never soften one — a healthy success ratio over the last hour does not
 *   cancel a fleet row saying the build is missing.
 * - **No reading is never healthy.** `no_opinion` survives as "No data", `quiet` stays
 *   muted, and an app with neither source answering reads as what it is — published —
 *   not as operational. `null` status keeps it between the verdicts and the drafts in
 *   `statusRank`, exactly where `statusOf` already put the unknown.
 * - **A draft serves nobody**, so nothing measured about it is a verdict on it.
 *
 * `source` names which input decided, so a surface can say "fleet health" or "last
 * hour's traffic" beside the label when that matters.
 */
export interface LifecycleVerdict {
  /** In the `AppStatus` vocabulary the fleet ranks on; `null` when nothing answered. */
  status: AppStatus | null;
  tone: AdminTone;
  label: string;
  source: "draft" | "fleet" | "availability" | "none";
}

export interface LifecycleVerdictInput {
  published: boolean;
  /**
   * The server's verdict for this app (`app.verdict`, read with the list). Used only when
   * `health` has not answered: the polled fleet-health row is fresher, and a stale list
   * row must not override it. `draft` is ignored here — `published` decides that, so a
   * stale row cannot un-publish an app on screen. Absent: the client mapping below
   * decides, exactly as it did before the field existed.
   */
  verdict?: AppStatus | null;
  /** The fleet-health row's verdict; `undefined`/`null` when the fleet has not answered. */
  health?: AppHealth | null;
  /** The per-app availability read; `undefined`/`null` when unconfigured (501) or failed. */
  availability?: Pick<AppAvailabilityReading, "verdict" | "severity"> | null;
}

function burningVerdict(
  availability: Pick<AppAvailabilityReading, "verdict" | "severity">
): LifecycleVerdict | null {
  if (availability.verdict !== "burning") return null;
  return availability.severity === "page"
    ? { status: "down", tone: "danger", label: "Burning", source: "availability" }
    : { status: "degraded", tone: "warn", label: "Degraded", source: "availability" };
}

export function lifecycleVerdict({
  published,
  verdict,
  health: fleetHealth,
  availability
}: LifecycleVerdictInput): LifecycleVerdict {
  if (!published) return { status: "draft", tone: "muted", label: "Draft", source: "draft" };

  // The polled fleet-health row first, the list row's server verdict when the fleet has
  // not answered. They are the same evaluator read at different times, so the later read
  // wins. `draft` here contradicts `published`, so it carries no health and falls through
  // to the client mapping.
  const health = fleetHealth ?? (verdict && verdict !== "draft" ? verdict : null);

  const burning = availability ? burningVerdict(availability) : null;

  if (health) {
    // `down` and `degraded` are already the worst the fleet can say; a burn verdict on
    // top changes nothing an operator would do differently.
    if (health === "down" || health === "degraded" || !burning) {
      return {
        status: health,
        tone: STATUS_TONE[health],
        label: STATUS_LABEL[health],
        source: "fleet"
      };
    }
    return burning;
  }

  if (availability) {
    if (burning) return burning;
    if (availability.verdict === "healthy") {
      return { status: "operational", tone: "ok", label: "Serving", source: "availability" };
    }
    return { status: "quiet", tone: "muted", label: "No data", source: "availability" };
  }

  return { status: null, tone: "muted", label: "Published", source: "none" };
}

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
