import { type ReactNode, useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogDestructiveAction,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { useAuth } from "@/contexts/AuthContext";
import {
  useDeleteBranch,
  useWorkspaceBranches as useProjectBranches,
  useSwitchWorkspaceBranch as useSwitchProjectBranch
} from "@/hooks/api/workspaces/useWorkspaces";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { detachedHeadLabel } from "@/libs/utils/detachedHead";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useIdeBranch from "@/stores/useIdeBranch";
import type { BranchRowData } from "./BranchRow";
import { BranchPopover } from "./index";

interface Props {
  trigger: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export function WorkspaceBranchSwitcher({ trigger, open, onOpenChange }: Props) {
  const { isLocalMode } = useAuth();
  const navigate = useNavigate();
  const { project, branchName: currentBranch } = useCurrentProjectBranch();
  const { setCurrentBranch, clearProjectBranch } = useIdeBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const projectId = project?.id || "";
  const { data: branchResponse, isLoading: isBranchesLoading } = useProjectBranches(projectId);
  const switchBranch = useSwitchProjectBranch();
  const deleteBranch = useDeleteBranch(projectId);

  const [switchingTo, setSwitchingTo] = useState<string | null>(null);
  const [branchPendingDelete, setBranchPendingDelete] = useState<string | null>(null);

  if (isLocalMode) return null;

  const activeBranchName = project?.active_branch?.name;
  // A detached working copy is on no branch, so the branch list has no row for
  // it. Give it one — labelled as what it is — so it stays reachable after the
  // IDE moves to a real branch.
  const detachedRow: BranchRowData | null =
    project?.detached_head && activeBranchName
      ? {
          name: activeBranchName,
          label: detachedHeadLabel(project.detached_head),
          showActiveBadge: activeBranchName !== currentBranch,
          canDelete: false
        }
      : null;
  const branchRows: BranchRowData[] = (branchResponse?.branches ?? []).map((b) => ({
    name: b.name,
    origin: b.origin,
    showActiveBadge: b.name === activeBranchName && b.name !== currentBranch,
    canDelete: b.name !== currentBranch && b.name !== activeBranchName
  }));
  const rows = detachedRow ? [detachedRow, ...branchRows] : branchRows;

  const handleSelect = async (branchName: string) => {
    if (branchName === currentBranch) {
      onOpenChange?.(false);
      return;
    }

    // Back to the detached working copy: nothing to switch on the server (it
    // is not a branch, and the server refuses it as one). Drop the IDE's
    // branch override so it follows the workspace's own HEAD again — never
    // store the label, which goes stale the moment HEAD moves.
    if (detachedRow && branchName === detachedRow.name) {
      clearProjectBranch(projectId);
      toast.success(`Switched to ${detachedRow.label}`);
      onOpenChange?.(false);
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.ROOT);
      return;
    }

    // Keep the popover open during the switch so the spinner stays visible;
    // close only on success so failures leave the picker available for retry.
    setSwitchingTo(branchName);
    try {
      await switchBranch.mutateAsync({ workspaceId: projectId, branchName });
      setCurrentBranch(projectId, branchName);
      toast.success(`Switched to "${branchName}"`);
      onOpenChange?.(false);
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.ROOT);
    } catch {
      toast.error("Failed to switch branch.");
    } finally {
      setSwitchingTo(null);
    }
  };

  const confirmDelete = async () => {
    const branchName = branchPendingDelete;
    if (!branchName) return;
    try {
      const result = await deleteBranch.mutateAsync(branchName);
      if (result.success) {
        toast.success(`Branch "${branchName}" deleted`);
        if (branchName === currentBranch) {
          const fallback =
            (branchResponse?.branches || []).find((b) => b.name !== branchName)?.name ??
            activeBranchName;
          if (fallback) {
            await switchBranch.mutateAsync({ workspaceId: projectId, branchName: fallback });
            setCurrentBranch(projectId, fallback);
            navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.ROOT);
          }
        }
      } else {
        toast.error(result.message || "Failed to delete branch");
      }
    } catch {
      toast.error("Failed to delete branch");
    } finally {
      setBranchPendingDelete(null);
    }
  };

  return (
    <>
      <BranchPopover
        trigger={trigger}
        open={open}
        onOpenChange={onOpenChange}
        branches={rows}
        activeBranch={currentBranch ?? ""}
        switchingTo={switchingTo}
        isLoading={isBranchesLoading}
        onSelect={(name) => void handleSelect(name)}
        onDelete={setBranchPendingDelete}
      />
      <AlertDialog
        open={branchPendingDelete !== null}
        onOpenChange={(next) => {
          if (!next) setBranchPendingDelete(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete branch?</AlertDialogTitle>
            <AlertDialogDescription>
              This permanently removes the local branch{" "}
              <code className='font-mono'>{branchPendingDelete}</code> and its worktree. The remote
              branch (if any) is untouched.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteBranch.isPending}>Cancel</AlertDialogCancel>
            <AlertDialogDestructiveAction onClick={confirmDelete} disabled={deleteBranch.isPending}>
              {deleteBranch.isPending ? "Deleting…" : "Delete branch"}
            </AlertDialogDestructiveAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
