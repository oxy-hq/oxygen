import { Panel, PanelContent, PanelHeader } from "@/components/ui/panel";
import type { ContextGraphNode } from "@/types/contextGraph";
import { TYPE_LABEL_SINGULAR } from "../constants";

interface NodeDetailPanelProps {
  node: ContextGraphNode | null;
  onClose: () => void;
}

export function NodeDetailPanel({ node, onClose }: NodeDetailPanelProps) {
  if (!node) return null;

  return (
    <Panel className='fixed top-0 right-0 z-50 w-96 border-l shadow-xl' animate>
      <PanelHeader
        title={node.label}
        subtitle={TYPE_LABEL_SINGULAR[node.type] || node.type}
        onClose={onClose}
      />
      <PanelContent>
        <div className='space-y-3'>
          {node.data.path && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Path</p>
              <p className='break-all rounded bg-muted px-2 py-1 font-mono text-xs'>
                {node.data.path}
              </p>
            </div>
          )}
          {node.data.description && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Description</p>
              <p className='text-sm'>{node.data.description}</p>
            </div>
          )}
          {node.data.database && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Database</p>
              <p className='font-mono text-sm'>{node.data.database}</p>
            </div>
          )}
          {node.data.datasource && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Datasource</p>
              <p className='font-mono text-sm'>{node.data.datasource}</p>
            </div>
          )}
        </div>
      </PanelContent>
    </Panel>
  );
}
