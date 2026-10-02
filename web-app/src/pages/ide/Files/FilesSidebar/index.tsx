import { FilePlus, Folder, FolderPlus, Layers2, Loader2, RotateCw } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { SidebarContent, SidebarGroup, SidebarMenu } from "@/components/ui/shadcn/sidebar";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import useFileTree from "@/hooks/api/files/useFileTree";
import useRepoFileTree from "@/hooks/api/repositories/useRepoFileTree";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { decodeBase64 } from "@/libs/encoding";
import { SidebarHeader } from "@/pages/ide/components/SidebarHeader";
import useSelectedRepo from "@/stores/useSelectedRepo";
import type { FileTreeModel, RepoSection } from "@/types/file";
import { useFilesContext } from "../FilesContext";
import { FilesSubViewMode, IGNORE_FILES_REGEX, NAME_COLLATOR } from "./constants";
import FileTreeNode from "./FileTreeNode";
import GroupedObjectsView from "./GroupedObjectsView";
import NewNode, { type CreationType } from "./NewNode";
import NewObjectButton from "./NewObjectButton";
import { getAllObjectFiles } from "./utils";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function githubUrlFromGitUrl(gitUrl: string): string | null {
  const match = gitUrl.match(/github\.com[/:]([^/]+\/[^/.]+?)(?:\.git)?$/);
  return match ? `https://github.com/${match[1]}` : null;
}

const GithubIcon = ({ className }: { className?: string }) => (
  <svg className={className} viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'>
    <path d='M12 2C6.477 2 2 6.477 2 12c0 4.418 2.865 8.166 6.839 9.489.5.092.682-.217.682-.482 0-.237-.009-.868-.013-1.703-2.782.603-3.369-1.342-3.369-1.342-.454-1.154-1.11-1.462-1.11-1.462-.908-.62.069-.608.069-.608 1.003.07 1.532 1.031 1.532 1.031.891 1.528 2.341 1.087 2.91.831.091-.645.349-1.087.635-1.337-2.22-.252-4.555-1.11-4.555-4.943 0-1.091.39-1.984 1.03-2.682-.103-.253-.447-1.27.097-2.646 0 0 .84-.269 2.75 1.025A9.578 9.578 0 0112 6.836a9.59 9.59 0 012.504.337c1.909-1.294 2.748-1.025 2.748-1.025.546 1.376.202 2.394.1 2.646.64.699 1.026 1.591 1.026 2.682 0 3.841-2.337 4.687-4.565 4.935.359.309.678.919.678 1.852 0 1.337-.012 2.414-.012 2.742 0 .267.18.578.688.48C19.138 20.163 22 16.418 22 12c0-5.523-4.477-10-10-10z' />
  </svg>
);

function prefixPaths(files: FileTreeModel[], prefix: string): FileTreeModel[] {
  return files.map((f) => ({
    ...f,
    path: `${prefix}/${f.path}`,
    children: prefixPaths(f.children, prefix)
  }));
}

// ─── Linked repo file tree ─────────────────────────────────────────────────────

const LinkedRepoFileTree: React.FC<{
  repo: RepoSection;
  activePath: string | undefined;
}> = ({ repo, activePath }) => {
  const {
    data: repoFiles = [],
    isFetching,
    refetch
  } = useRepoFileTree(repo.name, true, repo.sync_status);
  const isCloning = repo.sync_status === "cloning";
  const prevIsCloning = useRef(isCloning);

  // When cloning transitions to "ready", trigger one final fetch to pick up files immediately.
  useEffect(() => {
    if (prevIsCloning.current && !isCloning) {
      refetch();
    }
    prevIsCloning.current = isCloning;
  }, [isCloning, refetch]);

  const sortedFiles = useMemo(
    () =>
      prefixPaths(repoFiles, `@${repo.name}`)
        .filter((f) => !IGNORE_FILES_REGEX.some((r) => f.name.match(r)))
        .sort((a, b) => {
          if (a.is_dir && !b.is_dir) return -1;
          if (!a.is_dir && b.is_dir) return 1;
          return NAME_COLLATOR.compare(a.name, b.name);
        }),
    [repoFiles, repo.name]
  );

  if (isCloning && repoFiles.length === 0) {
    return (
      <div className='flex flex-col items-center gap-2 px-4 py-8 text-center'>
        <Loader2 className='h-5 w-5 animate-spin text-muted-foreground/40' />
        <p className='text-muted-foreground text-xs'>Cloning repository…</p>
        <p className='text-[11px] text-muted-foreground/50'>This may take a minute.</p>
      </div>
    );
  }

  if (!isCloning && repoFiles.length === 0 && !isFetching) {
    return (
      <div className='px-4 py-6 text-center text-muted-foreground text-xs'>No files found.</div>
    );
  }

  return (
    <SidebarMenu className='px-1 pb-4'>
      {sortedFiles.map((f) => (
        <FileTreeNode key={f.path} fileTree={f} activePath={activePath} />
      ))}
    </SidebarMenu>
  );
};

// ─── Main sidebar ─────────────────────────────────────────────────────────────

export const SIDEBAR_REVEAL_FILE = "sidebar:reveal-file";

const FilesSidebar: React.FC<{
  setSidebarOpen: (open: boolean) => void;
}> = ({ setSidebarOpen }) => {
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const { filesSubViewMode, setFilesSubViewMode } = useFilesContext();
  const { data, refetch, isPending } = useFileTree();
  const { pathb64 } = useParams();

  const { selectedRepo, setSelectedRepo } = useSelectedRepo();

  const repos = useMemo(() => data?.repositories ?? [], [data?.repositories]);

  // Reset to primary if the selected linked repo is removed
  useEffect(() => {
    if (selectedRepo !== "primary" && !repos.some((r) => r.name === selectedRepo)) {
      setSelectedRepo("primary");
    }
  }, [repos, selectedRepo, setSelectedRepo]);

  const activePath = useMemo(() => {
    if (!pathb64) return undefined;
    try {
      return decodeBase64(pathb64);
    } catch {
      return undefined;
    }
  }, [pathb64]);

  const [isCreating, setIsCreating] = useState(false);
  const [creationType, setCreationType] = useState<CreationType>("file");

  const handleCreateFile = () => {
    setCreationType("file");
    setIsCreating(true);
  };

  const handleCreateFolder = () => {
    setCreationType("folder");
    setIsCreating(true);
  };

  const fileTree = useMemo(() => {
    const filtered = data?.primary?.filter((f) => !IGNORE_FILES_REGEX.some((r) => f.name.match(r)));
    if (!filtered) return undefined;
    return filtered.sort((a, b) => {
      if (a.is_dir && !b.is_dir) return -1;
      if (!a.is_dir && b.is_dir) return 1;
      return NAME_COLLATOR.compare(a.name, b.name);
    });
  }, [data]);

  useEffect(() => {
    if (activePath) {
      try {
        window.dispatchEvent(
          new CustomEvent(SIDEBAR_REVEAL_FILE, { detail: { path: activePath } })
        );
      } catch {
        // ignore
      }
    }
  }, [activePath]);

  const isPrimary = selectedRepo === "primary";
  const selectedRepoData = isPrimary ? undefined : repos.find((r) => r.name === selectedRepo);

  // Poll the main file tree while any repo is still cloning so sync_status transitions to "ready".
  const isAnyCloning = repos.some((r) => r.sync_status === "cloning");
  useEffect(() => {
    if (!isAnyCloning) return;
    // TanStack's `refetch` resolves with the error rather than rejecting, so a failed
    // poll is simply retried on the next tick.
    const id = setInterval(() => {
      void refetch();
    }, 3_000);
    return () => clearInterval(id);
  }, [isAnyCloning, refetch]);

  // Poll primary file tree while empty AND a repo is still cloning — covers the case where
  // a GitHub-imported workspace is still being cloned and its files haven't appeared yet.
  // Guarded by isAnyCloning so a genuinely empty blank workspace doesn't trigger ~100 requests.
  const isPrimaryEmpty = !isPending && (data?.primary?.length ?? 0) === 0;
  const primaryPollCount = useRef(0);
  useEffect(() => {
    if (!isPrimaryEmpty || !isAnyCloning) {
      primaryPollCount.current = 0;
      return;
    }
    if (primaryPollCount.current >= 100) return; // safeguard: ~5 min at 3 s intervals
    const id = setInterval(() => {
      primaryPollCount.current += 1;
      refetch();
    }, 3_000);
    return () => clearInterval(id);
  }, [isPrimaryEmpty, isAnyCloning, refetch]);

  const linkedRepoGithubUrl = selectedRepoData?.git_url
    ? githubUrlFromGitUrl(selectedRepoData.git_url)
    : null;

  return (
    <div className='flex h-full flex-col overflow-hidden bg-sidebar-background'>
      <SidebarHeader
        title={
          <div className='flex items-center gap-1.5'>
            <span className='font-medium text-sidebar-foreground/80 text-xs'>
              {isPrimary ? "Files" : selectedRepo}
            </span>
            {!isPrimary && linkedRepoGithubUrl && (
              <a
                href={linkedRepoGithubUrl}
                target='_blank'
                rel='noopener noreferrer'
                title='Open on GitHub'
                onClick={(e) => e.stopPropagation()}
                className='flex items-center rounded p-0.5 text-muted-foreground/40 transition-colors hover:text-muted-foreground'
              >
                <GithubIcon className='h-3 w-3' />
              </a>
            )}
          </div>
        }
        onCollapse={() => setSidebarOpen(false)}
        actions={
          <>
            {isPrimary && (
              <>
                {filesSubViewMode === FilesSubViewMode.OBJECTS && <NewObjectButton />}
                {filesSubViewMode === FilesSubViewMode.FILES && (
                  <>
                    <Button
                      type='button'
                      variant='ghost'
                      size='sm'
                      onClick={handleCreateFile}
                      tooltip='New File'
                    >
                      <FilePlus className='h-4 w-4' />
                    </Button>
                    <Button
                      type='button'
                      variant='ghost'
                      size='sm'
                      onClick={handleCreateFolder}
                      tooltip='New Folder'
                    >
                      <FolderPlus className='h-4 w-4' />
                    </Button>
                  </>
                )}
              </>
            )}
            {isPrimary && filesSubViewMode === FilesSubViewMode.FILES && (
              <Button
                type='button'
                variant='ghost'
                size='sm'
                onClick={() => refetch()}
                tooltip='Refresh'
              >
                <RotateCw className='h-4 w-4' />
              </Button>
            )}
          </>
        }
      />

      {/* Objects / Files tabs — only for primary repo */}
      {isPrimary && (
        <div className='min-h-12.5 border-sidebar-border border-b px-2 py-2'>
          <Tabs
            value={filesSubViewMode}
            onValueChange={(v) => setFilesSubViewMode(v as FilesSubViewMode)}
            className='w-full'
          >
            <TabsList className='w-full'>
              <TabsTrigger value='objects'>
                <Layers2 />
                Objects
              </TabsTrigger>
              <TabsTrigger value='files'>
                <Folder />
                Files
              </TabsTrigger>
            </TabsList>
          </Tabs>
        </div>
      )}

      <SidebarContent className='customScrollbar h-full flex-1 overflow-y-auto'>
        {isPending && (
          <div className='flex items-center justify-center p-4'>
            <Spinner />
          </div>
        )}

        {!isPending && isPrimary && (
          <SidebarGroup className='px-1 pt-2'>
            {filesSubViewMode === FilesSubViewMode.OBJECTS && data && (
              <GroupedObjectsView
                files={getAllObjectFiles(
                  data.primary.filter((f) => !IGNORE_FILES_REGEX.some((r) => f.name.match(r)))
                )}
                projectId={projectId}
                activePath={activePath}
              />
            )}

            {filesSubViewMode === FilesSubViewMode.FILES && fileTree && (
              <SidebarMenu className='pb-2'>
                {isCreating && (
                  <NewNode
                    currentPath=''
                    creationType={creationType}
                    onCreated={() => {
                      setIsCreating(false);
                      refetch();
                    }}
                    onCancel={() => setIsCreating(false)}
                  />
                )}
                {fileTree.map((f) => (
                  <FileTreeNode key={f.path} fileTree={f} activePath={activePath} />
                ))}
              </SidebarMenu>
            )}
          </SidebarGroup>
        )}

        {!isPending && !isPrimary && selectedRepoData && (
          <LinkedRepoFileTree repo={selectedRepoData} activePath={activePath} />
        )}
      </SidebarContent>
    </div>
  );
};

export default FilesSidebar;
