import { ChevronDown } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useAuth } from "@/contexts/AuthContext";
import { useIdeGit } from "../../context/IdeGitContext";
import { BranchInfo } from "../BranchInfo";
import { WorkspaceBranchSwitcher } from "../BranchPopover/WorkspaceBranchSwitcher";
import { HistoryPopover } from "../HistoryPopover";
import { OpenPreviewButton } from "../OpenPreviewButton";
import { ActionsRow } from "./ActionsRow";

export function GitActions() {
  const { isLocalMode } = useAuth();
  const { workspaceId, branch, isOnMain, gitState, refresh } = useIdeGit();
  const [isBranchPickerOpen, setIsBranchPickerOpen] = useState(false);

  if (isLocalMode) return null;
  // Workspace has no git (git_mode === "none"); every capability is false.
  if (!gitState.caps.can_switch_branch) return null;

  const canBrowseHistory = !!gitState.caps.can_browse_history;

  const branchPill = (
    <Button size='sm' variant='outline'>
      <span className='min-w-0 flex-1'>
        <BranchInfo />
      </span>
      <ChevronDown />
    </Button>
  );

  return (
    <div className='flex items-center gap-1.5'>
      <WorkspaceBranchSwitcher
        trigger={branchPill}
        open={isBranchPickerOpen}
        onOpenChange={setIsBranchPickerOpen}
      />

      {/* Only a branch that is not the default one has anything to preview:
          the default branch IS what's live. Hides itself for non-staff. Keyed
          by branch so a switch mid-wait can't open the new branch instead. */}
      {workspaceId && branch && !isOnMain && (
        <OpenPreviewButton key={branch} workspaceId={workspaceId} branch={branch} />
      )}

      {canBrowseHistory && (
        <HistoryPopover workspaceId={workspaceId} branch={branch} onResetSuccess={refresh} />
      )}

      <ActionsRow />
    </div>
  );
}
