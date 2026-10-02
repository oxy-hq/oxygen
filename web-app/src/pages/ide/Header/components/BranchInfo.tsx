import { GitBranch } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import useRevisionInfo from "@/hooks/api/workspaces/useRevisionInfo";
import { detachedHeadFor, detachedHeadLabel } from "@/libs/utils/detachedHead";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { useIdeGit } from "../context/IdeGitContext";

export const BranchInfo = () => {
  const { isLocalMode } = useAuth();
  const { branch } = useIdeGit();
  const { workspace } = useCurrentWorkspace();
  // No branch to name: say so, rather than show the `HEAD@<sha>` label the
  // server uses in the branch slot.
  const detachedAt = detachedHeadFor(workspace, branch);
  // `isLoading` (not `isFetching`) — refetch on focus / poll / invalidate
  // would otherwise flash the skeleton every tick. React Query dedupes by
  // key with the provider's call.
  const { isLoading: revisionLoading } = useRevisionInfo(!isLocalMode);

  if (isLocalMode) return null;

  if (revisionLoading) {
    return (
      <div className='flex items-center gap-2'>
        <Spinner className='size-3 text-muted-foreground' />
        <Skeleton className='h-4 w-20 rounded' />
      </div>
    );
  }

  return (
    <div className='flex min-w-0 items-center gap-2'>
      <GitBranch className='h-3.5 w-3.5 flex-shrink-0 text-muted-foreground' />
      <span className='truncate font-mono text-sm' data-testid='ide-branch-name'>
        {detachedAt ? detachedHeadLabel(detachedAt) : branch || "No branch"}
      </span>
    </div>
  );
};
