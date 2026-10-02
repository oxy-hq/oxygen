import { ChevronDown, GitBranch, Loader2, Upload } from "lucide-react";
import { useState } from "react";
import useRepoBranch from "@/hooks/api/repositories/useRepoBranch";
import useRepoBranches from "@/hooks/api/repositories/useRepoBranches";
import useRepoCheckout from "@/hooks/api/repositories/useRepoCheckout";
import useRepoCommit from "@/hooks/api/repositories/useRepoCommit";
import useRepoDiff from "@/hooks/api/repositories/useRepoDiff";
import { ChangesPanel } from "./ChangesPanel";
import { RepoBranchSwitcher } from "./RepoBranchSwitcher";

export function LinkedRepoActions({ repoName }: { repoName: string }) {
  const { data: branchData } = useRepoBranch(repoName);
  const [changesPanelOpen, setChangesPanelOpen] = useState(false);
  const [branchOpen, setBranchOpen] = useState(false);
  const commit = useRepoCommit(repoName);
  const checkout = useRepoCheckout(repoName);
  const { data: branchesData, isFetching: branchesFetching } = useRepoBranches(
    repoName,
    branchOpen
  );
  const { data: diffData, refetch: refetchDiff } = useRepoDiff(repoName, true);

  const branch = branchData?.branch;
  const branches = branchesData?.branches ?? [];
  const changedFiles = diffData ?? [];
  const hasChanges = changedFiles.length > 0;

  const handleSelect = (b: string) => {
    if (b === branch || checkout.isPending) return;
    checkout.mutate(b);
  };

  const handleCommit = async (message: string) => {
    try {
      await commit.mutateAsync(message);
    } catch {
      // `useRepoCommit`'s `onError` already toasts the failure. Swallowed here because
      // `onPush` is not awaited by its caller, so a rethrow is an unhandled rejection.
    } finally {
      // A TanStack refetch resolves with the error rather than rejecting.
      void refetchDiff();
    }
  };

  const pill = (
    <button
      type='button'
      className='flex h-7 max-w-36 items-center gap-1.5 overflow-hidden rounded border border-border/50 bg-transparent px-2 text-sm transition-colors hover:border-border hover:bg-accent/40'
    >
      <span className='flex min-w-0 flex-1 items-center gap-1 font-mono text-muted-foreground text-xs'>
        {checkout.isPending ? (
          <Loader2 className='h-3 w-3 shrink-0 animate-spin' />
        ) : (
          <GitBranch className='h-3 w-3 shrink-0' />
        )}
        <span className='truncate'>{branch ?? "…"}</span>
      </span>
      <ChevronDown className='h-3 w-3 shrink-0 text-muted-foreground/60' />
    </button>
  );

  return (
    <div className='flex items-center gap-1.5'>
      <RepoBranchSwitcher
        trigger={pill}
        open={branchOpen}
        onOpenChange={setBranchOpen}
        branches={branches}
        currentBranch={branch}
        isLoading={branchesFetching && branches.length === 0}
        onSelect={handleSelect}
      />

      <div className='mx-0.5 h-4 w-px bg-border/50' />

      <button
        type='button'
        onClick={() => {
          void refetchDiff();
          setChangesPanelOpen(true);
        }}
        disabled={commit.isPending}
        className={`flex h-7 items-center gap-1 rounded px-2.5 font-medium text-xs transition-all ${
          hasChanges
            ? "bg-gradient-to-b from-[#3550FF] to-[#2A40CC] text-white shadow-[#0B1033]/40 shadow-sm hover:from-[#5D73FF] hover:to-[#3550FF]"
            : "border border-border/50 text-muted-foreground hover:border-border hover:bg-accent/40 hover:text-foreground"
        }`}
      >
        {commit.isPending ? (
          <Loader2 className='h-3 w-3 animate-spin' />
        ) : (
          <Upload className='h-3 w-3' />
        )}
        Commit
      </button>

      <ChangesPanel
        open={changesPanelOpen}
        onOpenChange={setChangesPanelOpen}
        isPushing={commit.isPending}
        pushLabel='Commit & Push'
        onPush={handleCommit}
      />
    </div>
  );
}
