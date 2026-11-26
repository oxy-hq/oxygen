import {
  Activity,
  Boxes,
  Database,
  FlaskConical,
  FolderGit2,
  Globe,
  Network,
  Radio
} from "lucide-react";
import type React from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { ThemeToggle } from "@/components/ThemeToggle";
import {
  DESIGN_REFERENCE,
  focusRing,
  sectionLinkOff,
  sectionLinkOn
} from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { useAuth } from "@/contexts/AuthContext";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

enum SidebarViewMode {
  WORLD_MODEL = "world-model",
  FILES = "files",
  DATABASE = "database",
  COORDINATOR = "coordinator",
  OBSERVABILITY = "observability",
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
  if (pathname.includes("/ide/database")) {
    return SidebarViewMode.DATABASE;
  }
  if (pathname.includes("/ide/coordinator")) {
    return SidebarViewMode.COORDINATOR;
  }
  if (pathname.includes("/ide/observability")) {
    return SidebarViewMode.OBSERVABILITY;
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
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const filesRoot = ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.ROOT;
  const currentViewMode = getViewModeFromPath(location.pathname, filesRoot);

  const handleNavigate = (mode: SidebarViewMode) => {
    switch (mode) {
      case SidebarViewMode.WORLD_MODEL:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.WORLD_MODEL.ROOT);
        break;
      case SidebarViewMode.FILES:
        navigate(filesRoot);
        break;
      case SidebarViewMode.DATABASE:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.DATABASE.ROOT);
        break;
      case SidebarViewMode.COORDINATOR:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.COORDINATOR.OVERVIEW);
        break;
      case SidebarViewMode.OBSERVABILITY:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACES);
        break;
      case SidebarViewMode.SEMANTIC:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.SEMANTIC.ROOT);
        break;
      case SidebarViewMode.EXPERIMENTS:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.EXPERIMENTS.ROOT);
        break;
      case SidebarViewMode.EDGE:
        navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.ROOT);
        break;
    }
  };

  return (
    <div className='flex h-full flex-col border-border-subtle border-r bg-sidebar-background'>
      <div className='flex flex-col items-center gap-1 px-1 py-2'>
        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.WORLD_MODEL)}
          data-testid='ide-nav-world-model'
          tooltip={{ content: "World Model", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.WORLD_MODEL ? sectionLinkOn : sectionLinkOff
          )}
        >
          <Globe className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.FILES)}
          data-testid='ide-nav-files'
          tooltip={{ content: "Files (read-only)", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.FILES ? sectionLinkOn : sectionLinkOff
          )}
        >
          <FolderGit2 className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.DATABASE)}
          data-testid='ide-nav-database'
          tooltip={{ content: "SQL client", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.DATABASE ? sectionLinkOn : sectionLinkOff
          )}
        >
          <Database className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.COORDINATOR)}
          tooltip={{ content: "Coordinator", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.COORDINATOR ? sectionLinkOn : sectionLinkOff
          )}
        >
          <Radio className='h-4 w-4' />
        </Button>

        {authConfig.enterprise && (
          <Button
            variant='ghost'
            size='icon'
            onClick={() => handleNavigate(SidebarViewMode.OBSERVABILITY)}
            tooltip={{ content: "Observability", side: "right", className: DESIGN_REFERENCE }}
            className={cn(
              "h-8 w-8",
              focusRing,
              currentViewMode === SidebarViewMode.OBSERVABILITY ? sectionLinkOn : sectionLinkOff
            )}
          >
            <Activity className='h-4 w-4' />
          </Button>
        )}

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.SEMANTIC)}
          data-testid='ide-nav-semantic'
          tooltip={{ content: "Semantic Model", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.SEMANTIC ? sectionLinkOn : sectionLinkOff
          )}
        >
          <Network className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.EXPERIMENTS)}
          data-testid='ide-nav-experiments'
          tooltip={{ content: "Experiments", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.EXPERIMENTS ? sectionLinkOn : sectionLinkOff
          )}
        >
          <FlaskConical className='h-4 w-4' />
        </Button>

        <Button
          variant='ghost'
          size='icon'
          onClick={() => handleNavigate(SidebarViewMode.EDGE)}
          tooltip={{ content: "Edge", side: "right", className: DESIGN_REFERENCE }}
          className={cn(
            "h-8 w-8",
            focusRing,
            currentViewMode === SidebarViewMode.EDGE ? sectionLinkOn : sectionLinkOff
          )}
        >
          <Boxes className='h-4 w-4' />
        </Button>
      </div>
      <div className='mt-auto flex flex-col items-center px-1 py-2'>
        <ThemeToggle
          align='start'
          side='right'
          className='text-subtle-foreground hover:bg-row-hover hover:text-foreground'
        />
      </div>
    </div>
  );
};

export default Sidebar;
