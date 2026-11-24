import { Loader2, MapPin, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useLocations } from "@/hooks/api/organizations";
import type { Organization, OrgRole } from "@/types/organization";
import NoAccessNotice from "../../../components/NoAccessNotice";
import { SectionActions } from "../../../components/SectionChrome";
import { LocationDialog } from "./components/LocationDialog";
import { LocationTableRow } from "./components/LocationTableRow";
import { locationSummary, locationTree } from "./utils";

interface LocationsSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

/**
 * The org's places, as a tree the tenant shapes: one self-reference and a
 * free-text kind per row, so "region > district > store" is three words the
 * tenant chose, not a schema. Copies Crew's shape: nav gate first, `canManage`
 * second, and it holds the query back so a deep link never reads on a
 * member's behalf.
 */
export default function LocationsSection({ org, viewerRole }: LocationsSectionProps) {
  const orgId = org.id;
  const canManage = viewerRole === "owner" || viewerRole === "admin";
  const locations = useLocations(orgId, canManage);
  const [creating, setCreating] = useState(false);
  const rows = useMemo(() => locationTree(locations.data ?? []), [locations.data]);

  if (!canManage) {
    return (
      <NoAccessNotice>
        You need to be an organization owner or admin to manage locations.
      </NoAccessNotice>
    );
  }

  const all = locations.data ?? [];

  return (
    <div className='flex flex-col gap-5' data-testid='settings-locations'>
      <SectionActions>
        <Button
          size='sm'
          className='gap-1.5'
          onClick={() => setCreating(true)}
          data-testid='settings-locations-new'
        >
          <Plus className='size-4' aria-hidden='true' />
          New location
        </Button>
      </SectionActions>

      {locations.data && (
        <p className='text-muted-foreground text-sm'>{locationSummary(locations.data)}</p>
      )}

      {locations.isPending ? (
        <div className='flex min-h-24 items-center justify-center'>
          <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
          <span className='sr-only'>Loading locations</span>
        </div>
      ) : locations.isError ? (
        <p className='py-8 text-center text-destructive text-sm'>Failed to load locations.</p>
      ) : all.length === 0 ? (
        <div className='flex flex-col items-center gap-3 rounded-md border py-10 text-center'>
          <MapPin className='h-8 w-8 text-muted-foreground/30' />
          <p className='max-w-sm text-muted-foreground text-sm'>
            No locations yet. Add the first store, then group stores under regions as you grow.
          </p>
          <Button
            size='sm'
            variant='outline'
            className='mt-1 gap-1.5'
            onClick={() => setCreating(true)}
            data-testid='settings-locations-new-empty'
          >
            <Plus className='h-4 w-4' />
            New location
          </Button>
        </div>
      ) : (
        <TableWrapper>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className='px-4'>Name</TableHead>
                <TableHead className='px-4'>Kind</TableHead>
                <TableHead className='px-4'>Status</TableHead>
                <TableHead className='px-4'>Timezone</TableHead>
                <TableHead className='px-4'>External ids</TableHead>
                <TableHead className='w-12' />
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map(({ location, depth }) => (
                <LocationTableRow
                  key={location.id}
                  orgId={orgId}
                  location={location}
                  depth={depth}
                  locations={all}
                />
              ))}
            </TableBody>
          </Table>
        </TableWrapper>
      )}

      <LocationDialog open={creating} onOpenChange={setCreating} orgId={orgId} locations={all} />
    </div>
  );
}
