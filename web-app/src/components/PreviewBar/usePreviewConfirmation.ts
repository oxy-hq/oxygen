import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useSyncExternalStore } from "react";
import queryKeys from "@/hooks/api/queryKey";
import { isPreviewRevisionServed, subscribePreviewServed } from "@/libs/utils/preview";
import { githubRepoBase } from "@/pages/ide/Header/hooks/useGithubUrls";
import type { RevisionInfo } from "@/types/settings";

/**
 * Whether the server has said it served this page from `revisionId`
 * (`x-oxy-preview: <branch>@<revision_id>`).
 *
 * The URL is what the page ASKED for; this is what it GOT. The bar only calls
 * a preview confirmed on the server's word, and against the revision itself —
 * another revision of the same branch is not a confirmation.
 */
export function useIsRevisionServed(revisionId: string): boolean {
  return useSyncExternalStore(subscribePreviewServed, () => isPreviewRevisionServed(revisionId));
}

/**
 * GitHub's compare view of the pinned commit against the default branch — but
 * only when the app already knows the repo.
 *
 * The remote URL comes from `revision-info`, an ide-only read the IDE header
 * makes on its own. Fetching it from a bar shown on every page would send
 * every pinned page load to the ide singleton for one link, so this only reads
 * what is already cached (any branch of this workspace; the remote is the same)
 * and leaves the link out otherwise. It compares the compiled SHA when known,
 * so the link shows what this revision contains, not where the branch is now.
 */
export function useKnownCompareUrl(
  workspaceId: string,
  target: { branch: string; sha: string | null },
  defaultBranch: string | undefined
): string | null {
  const queryClient = useQueryClient();
  const subscribe = useCallback(
    (onChange: () => void) => queryClient.getQueryCache().subscribe(onChange),
    [queryClient]
  );
  const remoteUrl = useSyncExternalStore(subscribe, () => {
    const cached = queryClient.getQueriesData<RevisionInfo>({
      queryKey: queryKeys.workspaces.revisionInfoAll(workspaceId)
    });
    return cached.find(([, info]) => !!info?.remote_url)?.[1]?.remote_url ?? null;
  });

  const base = githubRepoBase(remoteUrl);
  if (!base || !defaultBranch || target.branch === defaultBranch) return null;
  return `${base}/compare/${defaultBranch}...${target.sha ?? target.branch}`;
}
