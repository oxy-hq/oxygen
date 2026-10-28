import { Workflow as Automation, Bot } from "lucide-react";
import type { WorkspaceSummary } from "@/services/api/workspaces";

export function WorkspaceStats({ workspace }: { workspace: WorkspaceSummary }) {
  const { agent_count, workflow_count } = workspace;
  // A count is `null` when the instance that answered has no working copy to
  // count files in — it did not look, which is not the same as looking and
  // finding nothing. `?? 0` collapsed the two, so a workspace with seventeen
  // agents rendered exactly like an empty one: no stats row at all.
  const counted = agent_count != null || workflow_count != null;
  const hasAny = (agent_count ?? 0) > 0 || (workflow_count ?? 0) > 0;
  if (!counted) {
    return (
      <div
        className='flex items-center gap-3 pt-0.5 text-muted-foreground/50 text-xs'
        data-testid='workspace-stats-unknown'
        title='This instance has no working copy of the workspace, so it cannot count its agents or automations.'
      >
        <Bot className='size-3' />
        <span>—</span>
      </div>
    );
  }
  if (!hasAny) return null;

  return (
    <div className='flex items-center gap-3 pt-0.5'>
      {!!agent_count && (
        <span className='flex items-center gap-1 text-muted-foreground/50 text-xs'>
          <Bot className='size-3' />
          {agent_count}
        </span>
      )}
      {!!workflow_count && (
        <span className='flex items-center gap-1 text-muted-foreground/50 text-xs'>
          <Automation className='size-3' />
          {workflow_count}
        </span>
      )}
    </div>
  );
}
