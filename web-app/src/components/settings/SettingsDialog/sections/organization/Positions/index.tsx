import { BriefcaseBusiness, Loader2, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useAssignments, useOrgRoles } from "@/hooks/api/organizations";
import type { Organization, OrgRole } from "@/types/organization";
import NoAccessNotice from "../../../components/NoAccessNotice";
import { SectionActions } from "../../../components/SectionChrome";
import { NewPositionDialog } from "./components/NewPositionDialog";
import { OrgWidePane } from "./components/OrgWidePane";
import { PositionRow } from "./components/PositionRow";
import { holderCounts, positionSummary } from "./utils";

interface PositionsSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

/**
 * The org's position vocabulary (`org_roles`): the words a tenant uses for
 * what someone is called. Two stacked panes like Crew: the vocabulary, then
 * the org-wide positions with who holds each, because those have no location
 * dialog to be edited from.
 */
export default function PositionsSection({ org, viewerRole }: PositionsSectionProps) {
  const orgId = org.id;
  const canManage = viewerRole === "owner" || viewerRole === "admin";
  const roles = useOrgRoles(orgId, canManage);
  const assignments = useAssignments(orgId, {}, canManage);
  const [creating, setCreating] = useState(false);
  const counts = useMemo(
    () => (assignments.data ? holderCounts(assignments.data) : undefined),
    [assignments.data]
  );

  if (!canManage) {
    return (
      <NoAccessNotice>
        You need to be an organization owner or admin to manage positions.
      </NoAccessNotice>
    );
  }

  const sorted = [...(roles.data ?? [])].sort((a, b) => a.name.localeCompare(b.name));

  return (
    <div className='flex flex-col gap-5' data-testid='settings-positions'>
      <SectionActions>
        <Button
          size='sm'
          className='gap-1.5'
          onClick={() => setCreating(true)}
          data-testid='settings-positions-new'
        >
          <Plus className='size-4' aria-hidden='true' />
          New position
        </Button>
      </SectionActions>

      {roles.data && <p className='text-muted-foreground text-sm'>{positionSummary(roles.data)}</p>}

      {roles.isPending ? (
        <div className='flex min-h-24 items-center justify-center'>
          <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
          <span className='sr-only'>Loading positions</span>
        </div>
      ) : roles.isError ? (
        <p className='py-8 text-center text-destructive text-sm'>Failed to load positions.</p>
      ) : sorted.length === 0 ? (
        <div className='flex flex-col items-center gap-3 rounded-md border py-10 text-center'>
          <BriefcaseBusiness className='h-8 w-8 text-muted-foreground/30' />
          <p className='max-w-sm text-muted-foreground text-sm'>
            No positions yet. Add the ones your people hold, then assign them at each location.
          </p>
          <Button
            size='sm'
            variant='outline'
            className='mt-1 gap-1.5'
            onClick={() => setCreating(true)}
            data-testid='settings-positions-new-empty'
          >
            <Plus className='h-4 w-4' />
            New position
          </Button>
        </div>
      ) : (
        <TableWrapper>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className='px-4'>Name</TableHead>
                <TableHead className='px-4'>Scope</TableHead>
                <TableHead className='px-4'>Held by</TableHead>
                <TableHead className='w-12' />
              </TableRow>
            </TableHeader>
            <TableBody>
              {sorted.map((role) => (
                <PositionRow
                  key={role.id}
                  orgId={orgId}
                  role={role}
                  holders={counts ? (counts.get(role.id) ?? 0) : undefined}
                />
              ))}
            </TableBody>
          </Table>
        </TableWrapper>
      )}

      {roles.data && roles.data.length > 0 && (
        <OrgWidePane orgId={orgId} roles={roles.data} assignments={assignments.data ?? []} />
      )}

      <NewPositionDialog open={creating} onOpenChange={setCreating} orgId={orgId} />
    </div>
  );
}
