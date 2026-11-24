import { useMemo } from "react";
import { useAppEventTypes } from "@/hooks/api/customApps/useAppEvents";
import { useAppIssues } from "@/hooks/api/customApps/useAppIssues";
import type { CustomApp } from "@/types/apps";
import { liveIssueCount } from "../AppDetail/components/Issues/issueBrief";
import type { ConsolePageId } from "./consolePages";
import { eventsView } from "./Events/eventsView";

/**
 * The counts that appear beside a console page, in the one place that decides them.
 *
 * **A count here is always something wrong** — live issues on the build, dead event
 * deliveries. Never "how many functions this app has": a badge that counts inventory
 * makes every app look like it needs attention, and then a badge that means trouble
 * reads as inventory. A quiet app's nav is just names.
 *
 * Two surfaces draw this list — the console's own sidebar and the column browser's
 * Pages tier — so the read lives in a hook rather than in either of them. Both calls
 * hit the same query keys, so the two surfaces are one request and can never disagree
 * about whether an app has dead letters.
 */
export function useConsoleBadges(
  app: CustomApp,
  environment: string
): Partial<Record<ConsolePageId, number>> {
  const issues = useAppIssues(app.id);
  const types = useAppEventTypes(app.org_slug);
  const dead = useMemo(
    () => (types.data ? eventsView(types.data.types, app.slug, environment).totals.dead : 0),
    [types.data, app.slug, environment]
  );
  return {
    activity: issues.data ? liveIssueCount(issues.data.issues) : 0,
    events: dead
  };
}
