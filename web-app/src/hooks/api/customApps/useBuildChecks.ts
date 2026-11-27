import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AppBuildChecksService } from "@/services/api/appBuildChecks";
import type { BuildChecks, BuildChecksEnvironment } from "@/types/appBuildChecks";
import { buildChecksKeys } from "./buildChecksKeys";

/** How often a build whose checks are still running is re-read. */
export const BUILD_CHECKS_POLL_MS = 3_000;

/** How long after an attempt's last verdict the model's notes are still waited for. */
export const BUILD_CHECKS_NOTES_WAIT_MS = 120_000;

const isOpen = (status: string) => status === "queued" || status === "running";

/**
 * How soon to read a build's checks again, or `false` to stop.
 *
 * The roll-up settles as soon as the checks that decide it have a verdict, which is
 * earlier than the page is complete in two ways. A check that only informs (what
 * changed) can still be running. And on staging, an attempt that compared screens gets
 * the model's notes afterwards, the summary last: until it arrives, the attempt is read
 * for a bounded while after its last verdict, so an attempt that never gets one (no
 * notes task ran) stops being asked about.
 */
export function buildChecksPollMs(data: BuildChecks | undefined, now = Date.now()): number | false {
  if (!data) return false;
  if (data.state === "running" || data.checks.some((c) => isOpen(c.status))) {
    return BUILD_CHECKS_POLL_MS;
  }
  const notesFollow =
    data.environment !== "production" &&
    data.summary === null &&
    data.checks.some((c) => c.kind === "changes");
  if (!notesFollow) return false;
  const lastVerdict = Math.max(0, ...data.checks.map((c) => Date.parse(c.finished_at ?? "") || 0));
  return lastVerdict > 0 && now - lastVerdict < BUILD_CHECKS_NOTES_WAIT_MS
    ? BUILD_CHECKS_POLL_MS
    : false;
}

/**
 * One build's latest checks. `build` is the build's row id, so the Live build card and
 * the Releases page's Checks panel share one cache entry (and one poll). Re-read every
 * few seconds while anything is still to come ({@link buildChecksPollMs}), and not at
 * all after that.
 *
 * `environment` names the attempt: staging's (the default, asked for exactly as before
 * the production attempt existed) or the one run on production after a move, which
 * polls by the same rule.
 */
export function useBuildChecks(
  appId: string,
  build: string | undefined,
  environment: BuildChecksEnvironment = "staging"
) {
  const production = environment === "production";
  return useQuery({
    queryKey: production
      ? buildChecksKeys.buildChecksOnProduction(appId, build ?? "")
      : buildChecksKeys.buildChecks(appId, build ?? ""),
    queryFn: () =>
      production
        ? AppBuildChecksService.get(appId, build as string, "production")
        : AppBuildChecksService.get(appId, build as string),
    enabled: !!appId && !!build,
    refetchInterval: (query) => buildChecksPollMs(query.state.data)
  });
}

/**
 * "Run again": queue a new attempt of every check on `build`. A refusal (409
 * `build_not_on_staging` / `checks_running`) stays on the mutation's `error` for the
 * panel to say inline; a success refetches the app's checks, which then poll.
 */
export function useRerunBuildChecks(appId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (build: string) => AppBuildChecksService.rerun(appId, build),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: buildChecksKeys.appBuildChecks(appId) });
    }
  });
}

/**
 * "Mark as seen": a person has looked at the screens that differ from production. A
 * refusal (409 `nothing_to_see`) stays on the mutation's `error` for the row to say
 * inline; a success re-reads the app's checks, whose `changes` row then names who.
 */
export function useMarkChangesSeen(appId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ build, attempt }: { build: string; attempt: number }) =>
      AppBuildChecksService.markChangesSeen(appId, build, attempt),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: buildChecksKeys.appBuildChecks(appId) });
    }
  });
}
