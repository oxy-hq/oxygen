import { GitBranch } from "lucide-react";
import type { WorkspaceStatus, WorkspaceSummary } from "@/services/api/workspaces";

const STATUS_LABEL: Record<WorkspaceStatus, string> = {
  ready: "Ready",
  cloning: "Cloning…",
  failed: "Setup failed",
  not_oxy_project: "Not an Oxygen project"
};

const STATUS_CLASS: Record<WorkspaceStatus, string> = {
  ready: "text-muted-foreground",
  cloning: "text-muted-foreground",
  failed: "text-destructive",
  not_oxy_project: "text-warning"
};

function pluralize(n: number, singular: string, plural: string) {
  return `${n} ${n === 1 ? singular : plural}`;
}

function shortenRemote(url: string) {
  // Best-effort "owner/repo" form for display; fall back to the raw URL when
  // the remote isn't a recognizable git host URL.
  try {
    const withoutGit = url.replace(/\.git$/, "");
    const match = withoutGit.match(/[:/]([^/:]+\/[^/:]+)$/);
    return match ? match[1] : withoutGit;
  } catch {
    return url;
  }
}

export function WorkspaceTooltipContent({ workspace }: { workspace: WorkspaceSummary }) {
  const statusLabel = STATUS_LABEL[workspace.status];
  const statusClass = STATUS_CLASS[workspace.status];
  // Counts reflect parsed project contents — meaningless (and misleading) when
  // the clone failed, when there's no config.yml, or when the instance that
  // answered holds no working copy and returned `null` rather than counting.
  // That last case used to render "0 agents · 0 automations" for a
  // perfectly healthy workspace.
  const { agent_count, workflow_count } = workspace;
  const counted = agent_count !== null && workflow_count !== null;
  const showCounts =
    counted && workspace.status !== "failed" && workspace.status !== "not_oxy_project";
  return (
    <div className='flex flex-col gap-1.5'>
      <p className={`font-medium text-xs ${statusClass}`}>{statusLabel}</p>
      {workspace.error && <p className='break-words text-destructive text-xs'>{workspace.error}</p>}
      {showCounts && agent_count !== null && workflow_count !== null && (
        <p className='text-muted-foreground text-xs'>
          {pluralize(agent_count, "agent", "agents")} ·{" "}
          {pluralize(workflow_count, "automation", "automations")}
        </p>
      )}
      {workspace.git_remote && (
        <div className='flex items-center gap-1 text-muted-foreground text-xs'>
          <GitBranch className='h-3 w-3 shrink-0' />
          <span className='truncate'>{shortenRemote(workspace.git_remote)}</span>
        </div>
      )}
    </div>
  );
}
