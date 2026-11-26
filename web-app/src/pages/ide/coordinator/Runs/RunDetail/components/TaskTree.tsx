import { ChevronDown, ChevronRight, GitBranch } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { TaskTreeNode } from "@/services/api/coordinator";
import { StatusBadge } from "../../../components/StatusBadge";
import { formatDuration } from "../../../components/utils";
import { EventLog } from "./EventLog";

interface TreeNode extends TaskTreeNode {
  children: TreeNode[];
  depth: number;
}

/** Assemble the flat node list into a parent→child tree rooted at `rootId`. */
const buildTree = (nodes: TaskTreeNode[], rootId: string): TreeNode | null => {
  const byId = new Map<string, TaskTreeNode>();
  for (const n of nodes) byId.set(n.run_id, n);

  const childrenMap = new Map<string, TaskTreeNode[]>();
  for (const n of nodes) {
    if (n.parent_run_id) {
      const siblings = childrenMap.get(n.parent_run_id) ?? [];
      siblings.push(n);
      childrenMap.set(n.parent_run_id, siblings);
    }
  }

  const recurse = (id: string, depth: number): TreeNode | null => {
    const node = byId.get(id);
    if (!node) return null;
    const kids = (childrenMap.get(id) ?? [])
      .sort((a, b) => a.created_at.localeCompare(b.created_at))
      .map((c) => recurse(c.run_id, depth + 1))
      .filter(Boolean) as TreeNode[];
    return { ...node, children: kids, depth };
  };

  return recurse(rootId, 0);
};

const TreeNodeRow: React.FC<{ node: TreeNode }> = ({ node }) => {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  const hasDetail = !!node.answer || !!node.error_message || (node.event_log?.length ?? 0) > 0;

  return (
    <div>
      <div
        className={cn(
          "flex items-center gap-2 border-border border-b px-3 py-2 hover:bg-muted/50",
          node.depth === 0 && "bg-muted/30"
        )}
        style={{ paddingLeft: node.depth * 24 + 12 }}
      >
        <button
          type='button'
          onClick={() => setExpanded(!expanded)}
          className={cn(
            "flex h-5 w-5 shrink-0 items-center justify-center ",
            !hasChildren && "invisible"
          )}
        >
          {expanded ? (
            <ChevronDown className='h-3.5 w-3.5 text-muted-foreground' />
          ) : (
            <ChevronRight className='h-3.5 w-3.5 text-muted-foreground' />
          )}
        </button>
        {node.depth > 0 && <GitBranch className='h-3.5 w-3.5 shrink-0 text-muted-foreground' />}
        <div className='w-24 shrink-0'>
          <StatusBadge status={node.status} />
        </div>
        <div className='min-w-0 flex-1'>
          <p className='truncate text-sm'>{node.question}</p>
          <div className='mt-0.5 flex items-center gap-2'>
            {(node.agent_id || node.source_type) && (
              <span className='text-muted-foreground text-xs'>
                {node.agent_id || node.source_type}
              </span>
            )}
            {node.attempt > 0 && (
              <span className='text-warning text-xs'>attempt {node.attempt + 1}</span>
            )}
          </div>
        </div>
        <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
          {formatDuration(node.created_at, node.updated_at)}
        </span>
      </div>

      {expanded && hasDetail && (
        <div
          className='space-y-1.5 border-border border-b bg-muted/20 px-3 py-2'
          style={{ paddingLeft: node.depth * 24 + 48 }}
        >
          {node.event_log && node.event_log.length > 0 && <EventLog events={node.event_log} />}
          {node.error_message && (
            <div>
              <span className='font-medium text-destructive text-xs'>Error: </span>
              <span className='text-xs'>{node.error_message}</span>
            </div>
          )}
          {node.answer && (
            <div>
              <span className='font-medium text-muted-foreground text-xs'>Answer: </span>
              <span className='text-xs'>
                {node.answer.length > 400 ? `${node.answer.slice(0, 400)}…` : node.answer}
              </span>
            </div>
          )}
        </div>
      )}

      {expanded && node.children.map((child) => <TreeNodeRow key={child.run_id} node={child} />)}
    </div>
  );
};

/** The task graph for a run — the universal structure under every job type. */
export const TaskTree: React.FC<{ nodes: TaskTreeNode[]; rootId: string }> = ({
  nodes,
  rootId
}) => {
  const tree = useMemo(() => buildTree(nodes, rootId), [nodes, rootId]);
  if (!tree) {
    return <p className='px-3 py-6 text-muted-foreground text-sm'>No task tree available.</p>;
  }
  return <TreeNodeRow node={tree} />;
};
