import { Settings } from "lucide-react";
import { type ReactNode, useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Separator } from "@/components/ui/shadcn/separator";
import useSidebar from "@/components/ui/shadcn/sidebar-context";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { releaseBodyPointerLock } from "@/libs/utils/pointerEvents";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useManageWorkspacesDialog from "@/stores/useManageWorkspacesDialog";
import { WorkspaceRow } from "./WorkspaceRow";

type Props = {
  children: ReactNode;
};

export function WorkspaceSwitcherPopover({ children }: Props) {
  const [open, setOpen] = useState(false);
  const { workspace: currentWorkspace } = useCurrentWorkspace();
  const orgId = useCurrentOrg((s) => s.org?.id);
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { data: workspaces = [] } = useAllWorkspaces(orgId);
  const navigate = useNavigate();
  const { isMobile, setOpenMobile } = useSidebar();
  const openManageDialog = useManageWorkspacesDialog((s) => s.open);
  // Apps-first, this picker is the Factory's alone (the rail's opener is gone): it
  // keeps an Oxy engineer in the Factory on the other workspace, and offers no
  // "Manage sets of apps" — workspaces are created with `oxyc` and the API, and
  // deleted from the staff console.
  const appsFirst = useAppsFirstExperience();

  const handleSelect = (workspaceId: string) => {
    if (workspaceId === currentWorkspace?.id) {
      setOpen(false);
      return;
    }
    const target = workspaces.find((w) => w.id === workspaceId);
    if (!target?.org_id) {
      // List is cached per-org, so a missing org_id means the entry is stale
      // (probably mid-switch). Tell the user instead of silently swallowing
      // the click — the next list refresh will resolve it.
      toast.error("Workspace is unavailable. Try again in a moment.");
      return;
    }
    const targetWs = ROUTES.ORG(orgSlug).WORKSPACE(workspaceId);
    navigate(appsFirst ? targetWs.IDE.ROOT : targetWs.ROOT);
    setOpen(false);
  };

  const handleManageClick = () => {
    setOpen(false);
    // Close the mobile sidebar sheet so the manage page isn't hidden behind it.
    if (isMobile) setOpenMobile(false);
    // Radix Popover + Sheet both lock body pointer-events while closing.
    // Defer opening the dialog past the close animation and explicitly clear
    // the lock so the manage page is interactive when it mounts.
    requestAnimationFrame(() => {
      releaseBodyPointerLock();
      openManageDialog();
    });
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>{children}</PopoverTrigger>

      <PopoverContent side='bottom' align='start' sideOffset={4} className='w-56 p-1.5'>
        {workspaces.length > 0 && (
          <div className='mb-1'>
            {workspaces.map((w) => (
              <WorkspaceRow
                key={w.id}
                workspace={w}
                isActive={w.id === currentWorkspace?.id}
                onSelect={() => handleSelect(w.id)}
              />
            ))}
          </div>
        )}

        {!appsFirst && (
          <>
            <Separator className='my-1' />

            <button
              type='button'
              onClick={handleManageClick}
              className='flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-muted-foreground text-sm transition-colors hover:bg-accent hover:text-foreground'
            >
              <Settings className='h-3.5 w-3.5' />
              <span>Manage workspaces</span>
            </button>
          </>
        )}
      </PopoverContent>
    </Popover>
  );
}
