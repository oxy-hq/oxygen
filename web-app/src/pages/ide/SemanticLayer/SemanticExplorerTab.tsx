import { useState } from "react";
import { encodeBase64 } from "@/libs/encoding";
import { TopicExplorer } from "./explorer/Topic";
import { TopicExplorerProvider } from "./explorer/Topic/contexts/TopicExplorerContext";
import { ViewExplorer } from "./explorer/View";
import { ViewExplorerProvider } from "./explorer/View/contexts/ViewExplorerContext";
import SemanticObjectsList, { type SemanticObjectItem } from "./SemanticObjectsList";

/** Explorer tab: pick a topic/view from the sidebar, explore it inline. */
export default function SemanticExplorerTab() {
  const [selected, setSelected] = useState<SemanticObjectItem | null>(null);
  const pathb64 = selected ? encodeBase64(selected.path) : null;

  return (
    <div className='flex h-full min-h-0 flex-1'>
      <aside className='w-64 shrink-0 overflow-y-auto border-border border-r'>
        <SemanticObjectsList selectedPath={selected?.path ?? null} onSelect={setSelected} />
      </aside>
      <div className='flex min-h-0 min-w-0 flex-1 flex-col'>
        {!selected || !pathb64 ? (
          <p className='p-4 text-muted-foreground text-sm' data-testid='semantic-explorer-empty'>
            Select a topic or view from the sidebar to start exploring.
          </p>
        ) : selected.kind === "topic" ? (
          <TopicExplorerProvider pathb64={pathb64}>
            <TopicExplorer />
          </TopicExplorerProvider>
        ) : (
          <ViewExplorerProvider pathb64={pathb64}>
            <ViewExplorer />
          </ViewExplorerProvider>
        )}
      </div>
    </div>
  );
}
