import { ExternalLink, GitBranch } from "lucide-react";
import { CopyBlock } from "@/components/CopyBlock";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { gitCloneCommand, oxycCloneCommand, vscodeCloneUrl } from "./utils";

/**
 * The Factory's repository page, where the Files editor used to be: where the
 * workspace's repository is and how to open it in your own editor or with an
 * agent. Pushing to the default branch is how a change ships: it compiles from
 * git and deploys.
 */
const WorkspaceRepositoryPanel = () => {
  const { workspace } = useCurrentWorkspace();
  const remote = workspace?.git_remote_url ?? null;
  const branch = workspace?.default_branch || null;

  return (
    <div
      className='flex flex-1 justify-center overflow-auto p-6'
      data-testid='workspace-repository-panel'
    >
      <div className='flex w-full max-w-2xl flex-col gap-5'>
        <div className='flex flex-col gap-1'>
          <h1 className='font-semibold text-lg'>Work on this workspace in your editor</h1>
          <p className='text-muted-foreground text-sm'>
            Files are no longer edited here. The workspace is a git repository: change it with your
            own editor or an agent.
          </p>
        </div>
        {remote && workspace ? (
          <RepositoryDetails
            workspaceId={workspace.id}
            remote={remote}
            branch={branch}
            subdir={workspace.repo_subdir ?? null}
          />
        ) : (
          <NoRepository />
        )}
      </div>
    </div>
  );
};

const RepositoryDetails = ({
  workspaceId,
  remote,
  branch,
  subdir
}: {
  workspaceId: string;
  remote: string;
  branch: string | null;
  subdir: string | null;
}) => (
  <>
    <dl className='grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm'>
      <dt className='text-muted-foreground'>Repository</dt>
      <dd className='break-all font-mono' data-testid='retired-ide-repo-url'>
        {remote}
      </dd>
      {branch && (
        <>
          <dt className='text-muted-foreground'>Default branch</dt>
          <dd className='flex items-center gap-1 font-mono'>
            <GitBranch className='size-3.5' aria-hidden />
            {branch}
          </dd>
        </>
      )}
      {subdir && (
        <>
          <dt className='text-muted-foreground'>Folder</dt>
          <dd className='font-mono'>{subdir}/</dd>
        </>
      )}
    </dl>
    <div className='flex flex-col gap-2'>
      <span className='font-medium text-sm'>Clone it</span>
      <CopyBlock
        text={gitCloneCommand(remote, branch)}
        label='Copy git clone command'
        testId='retired-ide-git-clone'
        nowrap
      />
      <CopyBlock
        text={oxycCloneCommand(workspaceId)}
        label='Copy oxyc workspace clone command'
        testId='retired-ide-oxyc-clone'
        nowrap
      />
      <div>
        <Button asChild variant='outline' size='sm'>
          <a href={vscodeCloneUrl(remote)} data-testid='retired-ide-open-vscode'>
            <ExternalLink className='size-3.5' aria-hidden />
            Open in VS Code
          </a>
        </Button>
      </div>
    </div>
    <p className='text-muted-foreground text-sm'>
      Push to {branch ? <span className='font-mono'>{branch}</span> : "the default branch"} and the
      change compiles and deploys.
    </p>
  </>
);

const NoRepository = () => (
  <div className='rounded-md border p-4 text-sm' data-testid='retired-ide-no-repository'>
    <p className='font-medium'>This workspace has no repository.</p>
    <p className='text-muted-foreground'>
      Connect one in Settings, then clone it and work on it in your own editor.
    </p>
  </div>
);

export default WorkspaceRepositoryPanel;
