import { useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { DataFlow } from "@/types/dataFlow";
import { foldableGroupOf, foldableGroups, isGroup, viewFlow } from "../dataFlowView";
import type { DataFlowParams } from "../useDataFlowParams";
import { DataFlowGraph } from "./DataFlowGraph";
import { DataFlowViewBar } from "./DataFlowViewBar";
import { NodeDetails, NodeDetailsEmpty } from "./NodeDetails";

/**
 * The canvas and its details column over one answer, as the view folds and
 * filters it. Big groups start folded; a click opens one, and its members' details
 * offer to fold it again. The open groups live in the URL, so a view can be linked.
 */
export const DataFlowCanvas = ({
  flow,
  params,
  placeholder,
  onChange
}: {
  flow: DataFlow;
  params: DataFlowParams;
  /** The answer on screen is the last one while the next loads. */
  placeholder: boolean;
  onChange: (patch: Partial<DataFlowParams>) => void;
}) => {
  const [selected, setSelected] = useState<string | null>(null);
  const view = useMemo(
    () =>
      viewFlow(flow, { hidden: params.hide, search: params.q, open: params.open }, params.focus),
    [flow, params.hide, params.q, params.open, params.focus]
  );
  const selectedNode = view.nodes.find((n) => n.id === selected) ?? null;
  // A selection whose node folded away, or a filter hid, selects nothing: the canvas
  // stays undimmed instead of lighting the edges of a node it no longer draws.
  const shownSelected = selectedNode?.id ?? null;

  const openGroups = params.open;
  const expand = (group: string) => {
    if (openGroups !== "all" && !openGroups.includes(group)) {
      onChange({ open: [...openGroups, group] });
    }
  };
  const collapse = (group: string) => {
    // "All" is every group of the answer, not only those the filters leave on screen,
    // so folding one does not fold the ones a filter is hiding.
    const open = openGroups === "all" ? [...foldableGroups(flow.nodes).keys()] : openGroups;
    onChange({ open: open.filter((g) => g !== group) });
    setSelected(null);
  };
  // A details link can name a folded group; following it opens the group.
  const select = (id: string | null) => {
    const node = id ? view.nodes.find((n) => n.id === id) : undefined;
    if (node && isGroup(node)) expand(node.id);
    else setSelected(id);
  };
  const selectedGroup = shownSelected ? foldableGroupOf(view, shownSelected) : null;
  // Offered only when it would fold something: not for a group the focus or a match holds open.
  const canCollapse =
    selectedGroup !== null &&
    !view.held.has(selectedGroup) &&
    (openGroups === "all" || openGroups.includes(selectedGroup));

  return (
    <div className='flex flex-col gap-3'>
      <DataFlowViewBar params={params} foldable={view.groups.size} onChange={onChange} />
      <div
        className={cn(
          "flex min-w-0 overflow-hidden rounded-lg border border-border/60 bg-card",
          placeholder && "opacity-60"
        )}
      >
        {view.nodes.length === 0 ? (
          <p
            className='flex h-160 w-full items-center justify-center text-muted-foreground text-xs'
            data-testid='admin-data-flow-view-empty'
          >
            Nothing matches. Clear the search or show more kinds.
          </p>
        ) : (
          <DataFlowGraph
            flow={view}
            focus={params.focus}
            selected={shownSelected}
            matches={view.matches}
            onSelect={select}
            onExpand={expand}
          />
        )}
        {selectedNode ? (
          <NodeDetails
            flow={view}
            node={selectedNode}
            orgSlug={flow.org.slug}
            focused={selectedNode.id === params.focus}
            onFocus={(id) => {
              setSelected(null);
              onChange({ focus: id });
            }}
            onSelect={select}
            onCollapse={canCollapse && selectedGroup ? () => collapse(selectedGroup) : undefined}
          />
        ) : (
          <NodeDetailsEmpty />
        )}
      </div>
    </div>
  );
};
