import {
  Activity,
  Boxes,
  Database,
  FlaskConical,
  Folder,
  FolderGit2,
  Globe,
  Network,
  Radio
} from "lucide-react";
import type React from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { ThemeToggle } from "@/components/ThemeToggle";
import { Button } from "@/components/ui/shadcn/button";
import { useAuth } from "@/contexts/AuthContext";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import useIdeRetired from "@/hooks/useIdeRetired";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

enum SidebarViewMode {
  WORLD_MODEL = "world-model",
  FILES = "files",
  COORDINATOR = "coordinator",
  OBSERVABILITY = "observability",
  DATABASE = "database",
  SEMANTIC = "semantic",
  EXPERIMENTS = "experiments",
  EDGE = "edge"
}

const getViewModeFromPath = (pathname: string, filesRoot: string): SidebarViewMode => {
  if (pathname.includes("/ide/experiments")) {
    return SidebarViewMode.EXPERIMENTS;
  }
  if (pathname.includes("/ide/world-model")) {
    return SidebarViewMode.WORLD_MODEL;
  }
  if (pathname.includes("/ide/files") || pathname === filesRoot) {
    return SidebarViewMode.FILES;
  }
  if (pathname.includes("/ide/coordinator")) {
    return SidebarViewMode.COORDINATOR;
  }
  if (pathname.includes("/ide/observability")) {
    return SidebarViewMode.OBSERVABILITY;
  }
  if (pathname.includes("/ide/database")) {
    return SidebarViewMode.DATABASE;
  }
  if (pathname.includes("/ide/semantic")) {
    return SidebarViewMode.SEMANTIC;
  }
  // /ide/compliance is a legacy alias that still routes through the Edge
  // shell (see App.tsx redirect), so highlight the Edge button for it too.
  if (pathname.includes("/ide/edge") || pathname.includes("/ide/compliance")) {
    return SidebarViewMode.EDGE;
  }
  return SidebarViewMode.FILES;
};

const Sidebar: React.FC = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const { authConfig } = useAuth();
  // IDE retired: the Files slot opens the repository panel, and the Database
  // client goes.
  const ideRetired = useIdeRetired();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const filesRoot = ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.FILES.ROOT;
  const currentViewMode = getViewModeFromPath(location.pathname, filesRoot);

  const handleNavigate = (mode: SidebarViewMode) => {
    switch (mode) {
      case SidebarViewMode.WORLD_MODEL:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.WORLD_MODEL.ROOT);
        break;
      case SidebarViewMode.FILES:
        navigate(filesRoot);
        break;
      case SidebarViewMode.COORDINATOR:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.COORDINATOR.OVERVIEW);
        break;
      case SidebarViewMode.OBSERVABILITY:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.OBSERVABILITY.TRACES);
        break;
      case SidebarViewMode.DATABASE:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.DATABASE.ROOT);
        break;
      case SidebarViewMode.SEMANTIC:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.SEMANTIC.ROOT);
        break;
      case SidebarViewMode.EXPERIMENTS:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.EXPERIMENTS.ROOT);
        break;
      case SidebarViewMode.EDGE:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).IDE.EDGE.ROOT);
        break;
    }
  };

  return (
    <div className='flex h-full flex-col border-r bg-sidebar-background'>
      <div className='flex flex-col items-center gap-1 px-1 py-2'>
        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.WORLD_MODEL)}
          data-testid='ide-nav-world-model'
          tooltip={{ content: "World Model", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.WORLD_MODEL
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          <Globe className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.FILES)}
          data-testid='ide-nav-files'
          tooltip={{ content: ideRetired ? "Work in your editor" : "Files", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.FILES
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          {ideRetired ? <FolderGit2 className='h-4 w-4' /> : <Folder className='h-4 w-4' />}
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.COORDINATOR)}
          tooltip={{ content: "Coordinator", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.COORDINATOR
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          <Radio className='h-4 w-4' />
        </Button>

        {authConfig.enterprise && (
          <Button
            variant='ghost'
            size='icon'
            onClick={() => handleNavigate(SidebarViewMode.OBSERVABILITY)}
            tooltip={{ content: "Observability", side: "right" }}
            className={cn(
              "h-8 w-8",
              currentViewMode === SidebarViewMode.OBSERVABILITY
                ? "bg-sidebar-accent text-sidebar-accent-foreground"
                : "opacity-60 hover:opacity-100"
            )}
          >
            <Activity className='h-4 w-4' />
          </Button>
        )}

        {!ideRetired && (
          <Button
            variant='ghost'
            size='icon'
            onClick={() => handleNavigate(SidebarViewMode.DATABASE)}
            tooltip={{ content: "Database Client", side: "right" }}
            className={cn(
              "h-8 w-8",
              currentViewMode === SidebarViewMode.DATABASE
                ? "bg-sidebar-accent text-sidebar-accent-foreground"
                : "opacity-60 hover:opacity-100"
            )}
          >
            <Database className='h-4 w-4' />
          </Button>
        )}

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.SEMANTIC)}
          data-testid='ide-nav-semantic'
          tooltip={{ content: "Semantic Model", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.SEMANTIC
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          <Network className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.EXPERIMENTS)}
          data-testid='ide-nav-experiments'
          tooltip={{ content: "Experiments", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.EXPERIMENTS
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          <FlaskConical className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.EDGE)}
          tooltip={{ content: "Edge", side: "right" }}
          className={cn(
            "h-8 w-8",
            currentViewMode === SidebarViewMode.EDGE
              ? "bg-sidebar-accent text-sidebar-accent-foreground"
              : "opacity-60 hover:opacity-100"
          )}
        >
          <Boxes className='h-4 w-4' />
        </Button>
      </div>
      <div className='mt-auto flex flex-col items-center px-1 py-2'>
        <ThemeToggle align='start' side='right' className='opacity-60 hover:opacity-100' />
      </div>
    </div>
  );
};

export default Sidebar;
