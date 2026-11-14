import { Building2, Check, ChevronsUpDown } from "lucide-react";
import { useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { OpenFactoryDialog } from "@/components/admin/OpenFactoryDialog";
import { Button } from "@/components/ui/shadcn/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator
} from "@/components/ui/shadcn/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import { useOperatesPlatform } from "@/hooks/useStaffFactory";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { factoryPageIn } from "../factoryPageIn";

/** Past this many workspaces the list gets a search box. */
const SEARCH_FROM = 8;

/**
 * Which workspace the Factory is on, and the way to another.
 *
 * An org has one workspace, so for its own owners and admins this is just the name.
 * Oxy staff move between customers here: the trigger names the organization too,
 * and the list ends with "Another organization…", the Open Factory flow, which
 * starts the assumed session a customer's Factory needs. An org with several
 * workspaces lists them for everyone who can open its Factory. Picking one keeps
 * the section (`factoryPageIn`).
 */
export function WorkspaceSwitcher() {
  const { workspace } = useCurrentWorkspace();
  const org = useCurrentOrg((s) => s.org);
  const { data: workspaces = [] } = useAllWorkspaces(org?.id);
  const staff = useOperatesPlatform();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const [open, setOpen] = useState(false);
  const [otherOrgOpen, setOtherOrgOpen] = useState(false);

  const name = workspace?.name ?? "…";
  if (!staff && workspaces.length < 2) {
    return (
      <span className='max-w-48 truncate font-medium text-sm' data-testid='ide-workspace-name'>
        {name}
      </span>
    );
  }

  const orgRoutes = ROUTES.ORG(org?.slug ?? "");
  const currentRoot = workspace ? orgRoutes.WORKSPACE(workspace.id).IDE.ROOT : "";
  const goTo = (workspaceId: string) => {
    setOpen(false);
    if (workspaceId === workspace?.id) return;
    navigate(factoryPageIn(orgRoutes.WORKSPACE(workspaceId).IDE, currentRoot, pathname));
  };
  const openOtherOrg = () => {
    setOpen(false);
    setOtherOrgOpen(true);
  };

  return (
    <>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            variant='ghost'
            size='sm'
            className='h-8 min-w-0 max-w-96 gap-1.5 px-2 font-medium'
            aria-label={`Switch workspace (on ${name})`}
            data-testid='ide-workspace-switcher'
          >
            {staff && org?.name && (
              <>
                <span className='max-w-40 truncate text-muted-foreground'>{org.name}</span>
                <span className='text-muted-foreground' aria-hidden>
                  /
                </span>
              </>
            )}
            <span className='max-w-48 truncate' data-testid='ide-workspace-name'>
              {name}
            </span>
            <ChevronsUpDown className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
          </Button>
        </PopoverTrigger>
        <PopoverContent align='start' sideOffset={4} className='w-72 p-0'>
          <Command>
            {workspaces.length >= SEARCH_FROM && <CommandInput placeholder='Find a workspace…' />}
            <CommandList className='max-h-80'>
              <CommandEmpty>No workspace matches.</CommandEmpty>
              <CommandGroup heading={org?.name ?? "This organization"}>
                {workspaces.map((w) => (
                  <CommandItem
                    key={w.id}
                    value={`${w.name} ${w.id}`}
                    onSelect={() => goTo(w.id)}
                    data-testid={`ide-workspace-option-${w.id}`}
                  >
                    <span className='flex-1 truncate'>{w.name}</span>
                    {w.id === workspace?.id && <Check className='size-3.5 text-primary' />}
                  </CommandItem>
                ))}
              </CommandGroup>
              {staff && (
                <>
                  <CommandSeparator />
                  <CommandGroup>
                    <CommandItem
                      value='another organization'
                      onSelect={openOtherOrg}
                      data-testid='ide-open-other-org'
                    >
                      <Building2 className='text-muted-foreground' />
                      <span>Another organization…</span>
                    </CommandItem>
                  </CommandGroup>
                </>
              )}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
      {staff && <OpenFactoryDialog open={otherOrgOpen} onOpenChange={setOtherOrgOpen} />}
    </>
  );
}
