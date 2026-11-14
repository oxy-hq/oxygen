import { Home } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { GithubIcon } from "./components/GithubIcon";
import RevisionInfo from "./components/RevisionInfo";
import { WorkspaceSwitcher } from "./components/WorkspaceSwitcher";
import { githubRepoUrl } from "./githubRepoUrl";

/**
 * The Factory's top bar: home, the workspace (and, for Oxy staff, the way to
 * another workspace or organization: `WorkspaceSwitcher`), and what it is
 * serving (the branch and commit of the compiled revision, and when it
 * compiled), with the repository on GitHub at the right. There is no branch to
 * switch: a push compiles.
 */
const Header = () => {
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const navigate = useNavigate();

  const repoUrl = githubRepoUrl({
    remoteUrl: workspace?.git_remote_url,
    branch: workspace?.default_branch || "main",
    subdir: workspace?.repo_subdir
  });

  const handleHomeClick = () => {
    navigate(workspace?.id ? ROUTES.ORG(orgSlug).WORKSPACE(workspace.id).HOME : ROUTES.ROOT);
  };

  return (
    <header
      className='flex h-10 shrink-0 items-center gap-2 border-b bg-sidebar-background px-1'
      data-testid='ide-header'
    >
      <Button
        variant='ghost'
        size='sm'
        onClick={handleHomeClick}
        tooltip={{ content: "Back to Home", side: "right" }}
        className='h-8 w-8'
      >
        <Home className='h-4 w-4' />
      </Button>
      <WorkspaceSwitcher />
      <span className='h-4 w-px shrink-0 bg-border' aria-hidden />
      <RevisionInfo remoteUrl={workspace?.git_remote_url} />
      {repoUrl && (
        <Button
          asChild
          size='sm'
          variant='ghost'
          className='ml-auto h-8 w-8'
          title='Open on GitHub'
        >
          <a href={repoUrl} target='_blank' rel='noopener noreferrer' aria-label='Open on GitHub'>
            <GithubIcon className='h-3.5 w-3.5' />
          </a>
        </Button>
      )}
    </header>
  );
};

export default Header;
