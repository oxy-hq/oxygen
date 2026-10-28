import { FolderOpen } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/shadcn/command";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAllAdminOrgs } from "@/hooks/api/adminTenants/useAdminOrgs";
import {
  PICKER_PAGE_SIZE,
  useAdminWorkspacesForPicker,
  useAdminWorkspacesList
} from "@/hooks/api/adminTenants/useAdminWorkspaces";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import type { AssumeSession } from "@/types/adminAssume";

/** A workspace the "Open Factory…" dialog can open. */
export interface FactoryTarget {
  workspaceId: string;
  workspaceName: string;
  orgId: string;
  orgSlug: string;
  orgName: string;
}

/** The org a picker opened from inside (the rail in that org): its workspaces only. */
export interface PickerScope {
  orgId: string;
  orgSlug: string;
  orgName: string;
}

/**
 * Every org's workspaces grouped by org, filtered by what is typed (org or workspace
 * name). While acting as an org the staff console is closed, so the list is that
 * org's workspaces, read through the org's own route.
 *
 * With a `scope` (opened from inside an org), only that org's workspaces, with a way
 * to search every organization instead. An org with one workspace is picked at once.
 */
export function WorkspacePicker({
  actingSession,
  onPick,
  scope = null,
  onClearScope,
  autoPickSingle = true
}: {
  actingSession?: AssumeSession;
  onPick: (target: FactoryTarget) => void;
  scope?: PickerScope | null;
  onClearScope?: () => void;
  /** Pick a scoped org's only workspace at once (off after the person went Back). */
  autoPickSingle?: boolean;
}) {
  const acting = !!actingSession;
  const scoped = !acting && !!scope;
  const scopedList = useAdminWorkspacesList(
    { org_id: scope?.orgId, page_size: PICKER_PAGE_SIZE },
    { enabled: scoped }
  );
  // What is typed, and the same text a moment later for the server's search.
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  useEffect(() => {
    const timer = setTimeout(() => setSearch(query.trim()), 250);
    return () => clearTimeout(timer);
  }, [query]);
  // The newest page, filtered here by org or workspace name; and, once something is
  // typed, the server's search by workspace name across every workspace, so one
  // older than the newest page is still found.
  const newest = useAdminWorkspacesForPicker(!acting && !scoped);
  const searched = useAdminWorkspacesForPicker(!acting && !scoped && search !== "", search);
  const newestIsFull = (newest.data?.length ?? 0) >= PICKER_PAGE_SIZE;
  const everyWorkspace = useMemo(() => {
    if (scoped) {
      return {
        data: scopedList.data ?? [],
        isLoading: scopedList.isLoading,
        isError: scopedList.isError
      };
    }
    const rows = new Map((newest.data ?? []).map((w) => [w.id, w]));
    if (search) for (const w of searched.data ?? []) rows.set(w.id, w);
    return { data: [...rows.values()], isLoading: newest.isLoading, isError: newest.isError };
  }, [
    scoped,
    scopedList.data,
    scopedList.isLoading,
    scopedList.isError,
    newest.data,
    newest.isLoading,
    newest.isError,
    searched.data,
    search
  ]);
  const orgPages = useAllAdminOrgs(!acting);
  const assumedOrgWorkspaces = useAllWorkspaces(acting ? actingSession.org_id : undefined);

  const groups = useMemo(() => {
    const byOrg = new Map<string, { orgName: string; targets: FactoryTarget[] }>();
    if (acting) {
      const orgName = actingSession.org_name ?? actingSession.org_slug ?? "This organization";
      byOrg.set(actingSession.org_id, {
        orgName,
        targets: (assumedOrgWorkspaces.data ?? []).map((w) => ({
          workspaceId: w.id,
          workspaceName: w.name,
          orgId: actingSession.org_id,
          orgSlug: actingSession.org_slug ?? "",
          orgName
        }))
      });
    } else {
      const orgNames = new Map(
        (orgPages.data?.pages.flat() ?? []).map((o) => [o.id, o.name] as const)
      );
      for (const w of everyWorkspace.data ?? []) {
        if (!w.org_id || !w.org_slug) continue;
        const orgName =
          (scoped && w.org_id === scope?.orgId ? scope.orgName : undefined) ??
          orgNames.get(w.org_id) ??
          w.org_slug;
        const group = byOrg.get(w.org_id) ?? { orgName, targets: [] };
        group.targets.push({
          workspaceId: w.id,
          workspaceName: w.name,
          orgId: w.org_id,
          orgSlug: w.org_slug,
          orgName
        });
        byOrg.set(w.org_id, group);
      }
    }
    return [...byOrg.entries()]
      .map(([orgId, g]) => ({ orgId, ...g }))
      .sort((a, b) => a.orgName.localeCompare(b.orgName));
  }, [
    acting,
    actingSession,
    assumedOrgWorkspaces.data,
    everyWorkspace.data,
    orgPages.data,
    scoped,
    scope
  ]);

  // An org with one workspace has nothing to choose: go straight to it, once.
  const autoPicked = useRef(false);
  useEffect(() => {
    if (!scoped || !autoPickSingle || autoPicked.current || everyWorkspace.isLoading) return;
    const only =
      groups.length === 1 && groups[0].targets.length === 1 ? groups[0].targets[0] : null;
    if (only) {
      autoPicked.current = true;
      onPick(only);
    }
  }, [scoped, autoPickSingle, groups, everyWorkspace.isLoading, onPick]);

  const loading = acting ? assumedOrgWorkspaces.isLoading : everyWorkspace.isLoading;
  const failed = acting ? assumedOrgWorkspaces.isError : everyWorkspace.isError;

  return (
    <Command data-testid='open-factory-picker'>
      <CommandInput
        placeholder='Search organizations and workspaces…'
        value={query}
        onValueChange={setQuery}
        autoFocus
      />
      <CommandList className='max-h-96'>
        {loading ? (
          <div className='flex justify-center py-6'>
            <Spinner className='size-5' />
          </div>
        ) : failed ? (
          <p className='py-6 text-center text-destructive text-sm'>
            Couldn't load workspaces. Close this and try again.
          </p>
        ) : (
          <CommandEmpty>No organization or workspace matches.</CommandEmpty>
        )}
        {groups.map((group) => (
          <CommandGroup key={group.orgId} heading={group.orgName}>
            {group.targets.map((t) => (
              <CommandItem
                key={t.workspaceId}
                // What the search matches: both names, and the id for a pasted one.
                value={`${t.orgName} ${t.workspaceName} ${t.workspaceId}`}
                onSelect={() => onPick(t)}
                data-testid={`open-factory-item-${t.workspaceId}`}
              >
                <FolderOpen className='text-muted-foreground' />
                <span className='truncate'>{t.workspaceName}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        ))}
      </CommandList>
      {scoped && (
        <div className='flex items-center justify-between gap-2 border-t px-4 py-2 text-muted-foreground text-xs'>
          <span>{scope?.orgName}'s workspaces</span>
          <Button
            variant='ghost'
            size='sm'
            onClick={onClearScope}
            data-testid='open-factory-search-all'
          >
            Search all organizations
          </Button>
        </div>
      )}
      {!acting && !scoped && newestIsFull && !search && (
        <p className='border-t px-4 py-3 text-muted-foreground text-xs'>
          Showing the newest {PICKER_PAGE_SIZE} workspaces. Type a workspace name to search every
          one.
        </p>
      )}
      {acting && (
        <p className='border-t px-4 py-3 text-muted-foreground text-xs'>
          You're acting as {groups[0]?.orgName ?? "an organization"}. Stop that session to open
          another organization's Factory.
        </p>
      )}
    </Command>
  );
}
