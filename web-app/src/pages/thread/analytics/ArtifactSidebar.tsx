import type { ComponentProps } from "react";
import type { ArtifactItem, AutomationItem, SqlItem } from "@/hooks/analyticsSteps";
import AnalyticsArtifactSidebar from "./AnalyticsArtifactSidebar";
import { ReactQueryPanel, resultIdOf } from "./ReactQueryPanel";

type Props = Omit<ComponentProps<typeof AnalyticsArtifactSidebar>, "item"> & {
  /** The picked trace item and the run it belongs to. */
  selected: { runId: string; item: ArtifactItem | SqlItem | AutomationItem };
};

/**
 * The thread's side panel. A react run's `query` call whose result was stored opens
 * the stored result, paged from the server; everything else opens as before.
 */
export function ArtifactSidebar({ selected, ...rest }: Props) {
  const { item, runId } = selected;
  if (item.kind === "artifact" && item.toolName === "query" && item.queryExecuted) {
    const resultId = resultIdOf(item.toolOutput);
    if (resultId) {
      return (
        <ReactQueryPanel
          item={item.queryExecuted}
          resultId={resultId}
          runId={runId}
          onClose={rest.onClose}
        />
      );
    }
  }
  return <AnalyticsArtifactSidebar item={item} {...rest} />;
}
