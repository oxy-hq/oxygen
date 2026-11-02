import {
  type LifecycleVerdict,
  type LifecycleVerdictInput,
  lifecycleVerdict
} from "@/pages/admin/AdminCustomApps/appStatus";
import type { AppBuild, AppBuildHistory, CustomApp } from "@/types/apps";

/** The slice of an app the lifecycle reads. Every admin surface already holds a
 *  `CustomApp`; the dossier window holds the detail response. Both satisfy this. */
export type LifecycleAppRef = Pick<
  CustomApp,
  | "id"
  | "org_slug"
  | "slug"
  | "published_at"
  | "last_promoted_at"
  | "last_promoted_by_email"
  | "availability"
>;

export interface AppLifecycle {
  /** The build the published channel points at, or `null` when nothing is live. */
  liveBuild: AppBuild | null;
  /** The build the draft channel points at, or `null` when there is none. */
  draftBuild: AppBuild | null;
  /** The draft channel points at a build that is not the live one — something to promote. */
  draftAhead: boolean;
  /** `published_at` is set: customers can open the app. */
  published: boolean;
  publishedAt: string | null;
  verdict: LifecycleVerdict;
  /**
   * Writes staging held for the caller, or `null` when unknown — still loading, the
   * caller may not open this app's staging (404), or the read failed. A `null` here is
   * "no count", never "0".
   */
  heldCount: number | null;
  /** The held list hit its cap, so `heldCount` is a floor. */
  heldTruncated: boolean;
  /** Who last moved the live pointer, and when. From the build history when it has
   *  landed, else from the registry row. */
  lastPromoted: { at: string | null; byEmail: string | null };
  /** Every retained build, newest first — the "roll back to…" choices. */
  builds: AppBuild[];
  /** The build-history fetch: the one request without which there is no lifecycle. */
  isLoading: boolean;
  error: unknown;
}

/** How many held rows the console asks for — the list reads as a floor at this length.
 *  Mirrors `STAGING_HELD_LIMIT`, restated here so this module stays free of the
 *  service layer and testable without a window. */
const HELD_LIMIT = 100;

export interface DeriveLifecycleInput {
  app: LifecycleAppRef;
  history: AppBuildHistory | undefined;
  health: LifecycleVerdictInput["health"];
  availability: LifecycleVerdictInput["availability"];
  held: { writes: unknown[] }[] | null;
  heldUnavailable: boolean;
  isLoading: boolean;
  error: unknown;
}

/**
 * The pure derivation behind `useAppLifecycle`, kept apart from the hook so it can be
 * tested against real shapes without a QueryClient.
 */
export function deriveLifecycle({
  app,
  history,
  health,
  availability,
  held,
  heldUnavailable,
  isLoading,
  error
}: DeriveLifecycleInput): AppLifecycle {
  const builds = history?.builds ?? [];
  const liveBuild = builds.find((b) => b.is_published) ?? null;
  const draftBuild = builds.find((b) => b.is_draft) ?? null;
  const published = !!app.published_at;

  const heldCount =
    heldUnavailable || held === null ? null : held.reduce((n, e) => n + e.writes.length, 0);

  return {
    liveBuild,
    draftBuild,
    // A draft that *is* the live build is nothing to promote; the backend's `publish`
    // would be a no-op pointer move and the card would be offering it anyway.
    draftAhead: draftBuild !== null && (liveBuild === null || draftBuild.id !== liveBuild.id),
    published,
    publishedAt: app.published_at,
    verdict: lifecycleVerdict({ published, health, availability }),
    heldCount,
    heldTruncated: held !== null && held.length >= HELD_LIMIT,
    lastPromoted: {
      at: history?.promoted_at ?? app.last_promoted_at ?? null,
      byEmail: history?.promoted_by_email ?? app.last_promoted_by_email ?? null
    },
    builds,
    isLoading,
    error
  };
}
