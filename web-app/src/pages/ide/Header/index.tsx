import { Home } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Card } from "@/components/ui/shadcn/card";
import useRevisionInfo from "@/hooks/api/workspaces/useRevisionInfo";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useIdeBranch from "@/stores/useIdeBranch";
import { GitActions } from "./components/GitActions";
import { GithubIcon } from "./components/GithubIcon";
import { IDEProjectSwitcher } from "./components/IDEProjectSwitcher";
import { IdeGitProvider } from "./context/IdeGitContext";
import { useGithubUrls } from "./hooks/useGithubUrls";

// Stable reference so memoised consumers don't invalidate every render
// before the workspace loads.
const EMPTY_PROTECTED_BRANCHES: string[] = [];

const Header = () => {
  const { workspace: project } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const navigate = useNavigate();

  // `useCurrentWorkspaceBranch` throws without a workspace; Header renders
  // during initial load too, so resolve the branch manually.
  const { getCurrentBranch } = useIdeBranch();
  const activeBranch = project?.active_branch?.name ?? "";
  const ideBranch = project ? (getCurrentBranch(project.id) ?? activeBranch) : activeBranch;
  const defaultBranch = project?.default_branch ?? "main";
  const isOnMain = ideBranch === defaultBranch;

  const caps = project?.capabilities;
  const canDiff = !!caps?.can_diff;
  const protectedBranches = project?.protected_branches ?? EMPTY_PROTECTED_BRANCHES;

  // Header sits above <IdeGitProvider> so we read revisionInfo directly;
  // React Query dedupes by key so the provider's internal call shares it.
  const { data: revisionInfo } = useRevisionInfo(canDiff && !!project?.id);
  const { repoUrl: githubRepoUrl } = useGithubUrls({
    remoteUrl: revisionInfo?.remote_url,
    branch: ideBranch,
    defaultBranch,
    isOnMain
  });

  const handleHomeClick = () => {
    navigate(project?.id ? ROUTES.ORG(orgSlug).WORKSPACE(project.id).HOME : ROUTES.ROOT);
  };

  return (
    <Card className='flex gap-2 rounded-none border-y-0 border-t-0 border-b bg-sidebar-background p-1 shadow-none'>
      <Button
        variant='ghost'
        size='sm'
        onClick={handleHomeClick}
        tooltip={{ content: "Back to Home", side: "right" }}
        className='h-8 w-8'
      >
        <Home className='h-4 w-4' />
      </Button>
      <IDEProjectSwitcher />
      {githubRepoUrl && (
        <Button
          size='sm'
          onClick={(e) => {
            e.preventDefault();
            window.open(githubRepoUrl, "_blank", "noopener");
          }}
          title='Open on GitHub'
          variant='ghost'
        >
          <GithubIcon className='h-3.5 w-3.5' />
        </Button>
      )}
      <div className='flex flex-1 items-center justify-end gap-2'>
        <IdeGitProvider
          workspaceId={project?.id}
          branch={ideBranch}
          defaultBranch={defaultBranch}
          isOnMain={isOnMain}
          protectedBranches={protectedBranches}
          capabilities={project?.capabilities}
        >
          <GitActions />
        </IdeGitProvider>
      </div>
    </Card>
  );
};

export default Header;
