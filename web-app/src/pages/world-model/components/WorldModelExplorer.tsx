import { useState } from "react";
import { WorldModelGraph } from "@/pages/ide/WorldModel/components/WorldModelGraph";
import type { WmSelection, WorldModel } from "@/types/worldModel";
import { EntityDetails } from "./EntityDetails";

/**
 * The graph and, beside it (below it on a phone), what is selected. Only entity and
 * connection selections: no instance picker, no breakdowns, no sample browsing.
 * Those read routes that stay the Factory's.
 */
export function WorldModelExplorer({ model }: { model: WorldModel }) {
  const [selection, setSelection] = useState<WmSelection>(null);

  return (
    <div className='flex min-h-0 flex-1 flex-col lg:flex-row'>
      <div className='relative h-96 min-h-0 shrink-0 lg:h-auto lg:flex-1'>
        <WorldModelGraph
          model={model}
          selection={selection}
          plain
          onSelectEntity={(entityId) => setSelection({ kind: "entity", entityId })}
          onSelectPromotion={(from, to) => setSelection({ kind: "promotion", from, to })}
          onClearSelection={() => setSelection(null)}
        />
      </div>
      <aside className='shrink-0 border-t lg:w-80 lg:overflow-auto lg:border-t-0 lg:border-l'>
        <EntityDetails
          model={model}
          selection={selection}
          onSelectEntity={(entityId) => setSelection({ kind: "entity", entityId })}
        />
      </aside>
    </div>
  );
}
