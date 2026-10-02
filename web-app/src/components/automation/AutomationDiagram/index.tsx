import type { ReactFlowInstance } from "@xyflow/react";
import { Background, BackgroundVariant, type ColorMode, Controls, ReactFlow } from "@xyflow/react";
import type React from "react";
import { useRef } from "react";
import LoadingSkeleton from "@/components/ui/LoadingSkeleton";
import useAutomation, { type AutomationConfig, type NodeType } from "@/stores/useAutomation";
import useTheme from "@/stores/useTheme";
import { DiagramNode } from "./DiagramNode";
import { usePersistedViewport } from "./hooks/usePersistedViewport";
import { useAutomationLayout } from "./layout/useAutomationLayout";
import { restoreOrFit } from "./utils/viewport";

const nodeTypes: Record<NodeType, typeof DiagramNode> = {
  agent: DiagramNode,
  execute_sql: DiagramNode,
  semantic_query: DiagramNode,
  loop_sequential: DiagramNode,
  formatter: DiagramNode,
  workflow: DiagramNode,
  conditional: DiagramNode,
  "conditional-else": DiagramNode,
  "conditional-if": DiagramNode,
  omni_query: DiagramNode,
  looker_query: DiagramNode,
  airway: DiagramNode
} as const;

interface AutomationDiagramProps {
  automationId: string;
  runId?: string;
  automationConfig: AutomationConfig;
}

const AutomationDiagram: React.FC<AutomationDiagramProps> = ({
  automationId,
  runId,
  automationConfig
}) => {
  const onNodesChange = useAutomation((state) => state.onNodesChange);
  const onEdgesChange = useAutomation((state) => state.onEdgesChange);
  const { nodes, edges, fitViewOptions, layoutFailed } = useAutomationLayout(
    automationId,
    automationConfig.tasks,
    runId
  );

  const reactFlowRef = useRef<ReactFlowInstance | null>(null);
  const { load: loadSavedViewport, save: saveViewport } = usePersistedViewport(
    `oxy.automation.viewport.${automationId}`
  );

  const { theme } = useTheme();

  if (layoutFailed) {
    return (
      <div className='flex h-full w-full items-center justify-center text-destructive text-sm'>
        Failed to draw the automation diagram
      </div>
    );
  }

  if (nodes.length === 0) {
    return <LoadingSkeleton />;
  }

  return (
    <div className='h-full w-full'>
      <ReactFlow
        key={automationId}
        onInit={(instance) => {
          reactFlowRef.current = instance as unknown as ReactFlowInstance;
          const saved = loadSavedViewport();
          restoreOrFit(instance as unknown as ReactFlowInstance, saved, fitViewOptions);
        }}
        onMoveEnd={(..._args: unknown[]) => {
          const viewport = _args[1] as { x: number; y: number; zoom: number } | undefined;
          saveViewport(viewport);
        }}
        colorMode={theme as ColorMode}
        nodeTypes={nodeTypes}
        proOptions={{ hideAttribution: true }}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        nodes={nodes}
        edges={edges}
        draggable={false}
        nodesDraggable={false}
      >
        <Controls showInteractive={false} />
        <Background
          color='var(--border)'
          bgColor='var(--background)'
          variant={BackgroundVariant.Dots}
        />
      </ReactFlow>
    </div>
  );
};

export default AutomationDiagram;
