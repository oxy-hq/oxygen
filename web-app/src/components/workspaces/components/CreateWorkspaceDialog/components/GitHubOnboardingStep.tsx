import { AlertTriangle, FolderOpen, Loader2, RefreshCw } from "lucide-react";
import { useState } from "react";
import { GitNamespaceSelection } from "@/components/GitNamespaceSelection";
import { Button } from "@/components/ui/shadcn/button";
import { Combobox } from "@/components/ui/shadcn/combobox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useGitHubRepositoriesWithApp } from "@/hooks/api/github";
import { OnboardingService } from "@/services/api";
import useCurrentOrg from "@/stores/useCurrentOrg";

interface Props {
  onBack: () => void;
  onDone: (projectId: string) => void;
  projectName?: string;
}

export const GitHubOnboardingStep = ({ onBack, onDone, projectName }: Props) => {
  const { org } = useCurrentOrg();
  const orgId = org?.id ?? "";
  const [namespaceId, setNamespaceId] = useState("");
  const [repoId, setRepoId] = useState<number | null>(null);
  // The repository's default branch: what the workspace serves and a push
  // compiles. Sent for servers that still read it; there is no picker.
  const [branch, setBranch] = useState("");
  const [subdir, setSubdir] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // localName: user-editable project name; null means "follow repo name"
  const [localName, setLocalName] = useState<string>(projectName ?? "");
  const [nameAutoFilled, setNameAutoFilled] = useState(!projectName);

  const {
    data: repositories = [],
    isPending: isLoadingRepos,
    error: repoError,
    refetch: refetchRepos,
    isFetching: isFetchingRepos
  } = useGitHubRepositoriesWithApp(orgId, namespaceId);

  const repoErrorStatus = (repoError as { response?: { status?: number } } | null)?.response
    ?.status;
  const isRepoAuthError = repoErrorStatus === 403 || repoErrorStatus === 404;
  const isEmptyRepos = !isLoadingRepos && !isRepoAuthError && repositories.length === 0;

  const handleRepoChange = (value: string) => {
    const repo = repositories.find((r) => String(r.id) === value);
    if (repo) {
      setRepoId(repo.id);
      setBranch(repo.default_branch);
      // Auto-fill the project name from the repo short name if the user hasn't typed one
      if (nameAutoFilled) {
        setLocalName(repo.name);
      }
    }
  };

  const handleNameChange = (value: string) => {
    setLocalName(value);
    setNameAutoFilled(false);
  };

  const handleSubmit = async () => {
    if (!namespaceId || repoId === null || !branch) return;
    setIsSubmitting(true);
    setError(null);
    try {
      if (!org?.id) {
        setError("Select an organization first.");
        setIsSubmitting(false);
        return;
      }
      const result = await OnboardingService.setupGitHub(
        org.id,
        namespaceId,
        repoId,
        branch,
        localName.trim() || undefined,
        subdir.trim() || undefined
      );
      onDone(result.workspace_id);
    } catch (err) {
      const response = (err as { response?: { data?: unknown; status?: number } })?.response;
      const body = response?.data;
      const msg =
        typeof body === "string" && body.length > 0
          ? body
          : response?.status === 409
            ? "A workspace with that name already exists. Please choose a different name."
            : "Failed to import repository";
      setError(msg);
      setIsSubmitting(false);
    }
  };

  const canSubmit = !!namespaceId && repoId !== null && !!branch && !isSubmitting;

  const repoItems = repositories.map((r) => ({
    value: String(r.id),
    label: r.full_name,
    searchText: r.full_name
  }));

  return (
    <div className='flex flex-col gap-6'>
      <GitNamespaceSelection value={namespaceId} onChange={setNamespaceId} />

      {namespaceId && (
        <div className='space-y-2'>
          <div className='flex items-center justify-between'>
            <Label>Repository</Label>
            <button
              type='button'
              onClick={() => refetchRepos()}
              disabled={isFetchingRepos}
              className='flex items-center gap-1 text-muted-foreground/50 text-xs transition-colors hover:text-muted-foreground disabled:opacity-40'
              title='Refresh repository list'
            >
              <RefreshCw className={`h-3 w-3 ${isFetchingRepos ? "animate-spin" : ""}`} />
              Refresh
            </button>
          </div>
          {isLoadingRepos ? (
            <div className='flex h-9 items-center gap-2 rounded-md border border-input bg-input/30 px-3 text-muted-foreground text-sm'>
              <Loader2 className='h-3.5 w-3.5 animate-spin' />
              Loading repositories…
            </div>
          ) : isRepoAuthError ? (
            <div className='flex items-start gap-2.5 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2.5 dark:border-amber-800/40 dark:bg-amber-950/20'>
              <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400' />
              <div className='min-w-0 flex-1'>
                <p className='text-amber-700 text-xs leading-snug dark:text-amber-400'>
                  {repoErrorStatus === 404
                    ? "GitHub App installation not found. It may have been uninstalled."
                    : "Could not access GitHub. The token or app permissions may have expired."}
                </p>
                <button
                  type='button'
                  onClick={() => setNamespaceId("")}
                  className='mt-1 text-amber-700 text-xs underline underline-offset-2 hover:text-amber-900 dark:text-amber-400 dark:hover:text-amber-200'
                >
                  Reconnect account
                </button>
              </div>
            </div>
          ) : isEmptyRepos ? (
            <div className='flex items-start gap-2.5 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2.5 dark:border-amber-800/40 dark:bg-amber-950/20'>
              <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400' />
              <div className='min-w-0 flex-1'>
                <p className='text-amber-700 text-xs leading-snug dark:text-amber-400'>
                  No repositories found. The GitHub App may not have access to any repositories.
                  Reinstall the GitHub App to grant repository access.
                </p>
              </div>
            </div>
          ) : (
            <Combobox
              items={repoItems}
              value={repoId !== null ? String(repoId) : ""}
              onValueChange={handleRepoChange}
              placeholder='Select repository'
              searchPlaceholder='Search repositories…'
            />
          )}
        </div>
      )}

      {namespaceId && repoId !== null && branch && (
        <p className='text-muted-foreground text-sm' data-testid='github-import-branch'>
          Serves the default branch, <span className='font-mono text-foreground'>{branch}</span>. A
          push to it compiles.
        </p>
      )}

      {namespaceId && repoId !== null && (
        <div className='space-y-2'>
          <div className='flex items-center gap-1.5'>
            <Label htmlFor='github-subdir'>Subdirectory</Label>
            <span className='rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground'>
              optional
            </span>
          </div>
          <div className='relative'>
            <FolderOpen className='pointer-events-none absolute top-1/2 left-3 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground/50' />
            <Input
              id='github-subdir'
              placeholder='e.g. analytics/oxy'
              value={subdir}
              onChange={(e) => setSubdir(e.target.value)}
              className='pl-8 font-mono text-sm'
            />
          </div>
          <p className='text-muted-foreground text-xs'>
            Leave blank to use the repository root. For monorepos, enter the path to the Oxygen
            project folder.
          </p>
        </div>
      )}

      {namespaceId && repoId !== null && (
        <div className='space-y-2'>
          <Label htmlFor='github-workspace-name'>Workspace name</Label>
          <Input
            id='github-workspace-name'
            placeholder='my-workspace'
            value={localName}
            onChange={(e) => handleNameChange(e.target.value)}
            className='font-mono text-sm'
          />
          <p className='text-muted-foreground text-xs'>
            The workspace's display name. Leave blank to use the repository name.
          </p>
        </div>
      )}

      {error && <p className='text-destructive text-sm'>{error}</p>}

      <div className='flex gap-3'>
        <Button variant='outline' onClick={onBack} disabled={isSubmitting}>
          Back
        </Button>
        <Button onClick={handleSubmit} disabled={!canSubmit} className='flex-1'>
          {isSubmitting && <Loader2 className='mr-2 h-4 w-4 animate-spin' />}
          Import Repository
        </Button>
      </div>
    </div>
  );
};
