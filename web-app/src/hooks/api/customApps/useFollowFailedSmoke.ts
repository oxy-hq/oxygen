import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import type { BuildChecks } from "@/types/appBuildChecks";
import type { CustomApp } from "@/types/apps";
import queryKeys from "../queryKey";

/** When, after production's check fails, the app's builds are read again. The rollback
 *  is a queued task: a few seconds as a rule, longer behind a busy worker. */
export const FAILED_SMOKE_REREADS_MS = [3_000, 8_000, 20_000, 40_000] as const;

/** A failure older than this was acted on already, or will not be. */
export const FAILED_SMOKE_FRESH_MS = 60_000;

/** When production's own check of the build failed; null while it has not. */
export function failedSmokeAt(production: BuildChecks | undefined): string | null {
  const row = production?.checks.find((c) => c.kind === "production_smoke");
  return row?.status === "failed" ? row.finished_at : null;
}

/**
 * Follow a production move nobody on this page made.
 *
 * When production's own check of the live build fails and the app rolls back by itself,
 * the platform moves production a few seconds after the verdict. A move made here
 * re-reads the app's builds when it succeeds (`useMoveProduction`); this one has no
 * mutation to hang that on, so an open card kept naming the build that was just
 * replaced and offered a rollback that had already happened. The builds are read again
 * a few times after a verdict that just arrived, and never for an old one.
 *
 * `production` is the production attempt of the build the app serves; nothing is
 * followed for an app that does not ask for the automatic rollback.
 */
export function useFollowFailedSmoke(
  app: Pick<CustomApp, "id" | "promotion_checks">,
  production: BuildChecks | undefined
) {
  const queryClient = useQueryClient();
  const appId = app.id;
  const rollsBack = app.promotion_checks?.auto_rollback === true;
  const failedAt = failedSmokeAt(production);
  useEffect(() => {
    if (!rollsBack || !failedAt) return;
    const age = Date.now() - Date.parse(failedAt);
    if (!(age < FAILED_SMOKE_FRESH_MS)) return;
    const timers = FAILED_SMOKE_REREADS_MS.map((ms) =>
      setTimeout(() => {
        void queryClient.invalidateQueries({ queryKey: queryKeys.customApps.all() });
        void queryClient.invalidateQueries({ queryKey: queryKeys.customApps.builds(appId) });
      }, ms)
    );
    return () => {
      for (const timer of timers) clearTimeout(timer);
    };
  }, [appId, failedAt, rollsBack, queryClient]);
}
