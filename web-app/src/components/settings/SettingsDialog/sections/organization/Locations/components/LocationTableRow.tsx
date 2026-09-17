import { CornerDownRight } from "lucide-react";
import { Badge } from "@/components/ui/shadcn/badge";
import { TableCell, TableRow } from "@/components/ui/shadcn/table";
import type { LocationRow } from "@/types/operatingGraph";
import { LocationRowActions } from "./LocationRowActions";
import { LocationStatusBadge } from "./LocationStatusBadge";

const CELL = "px-4 py-3 max-md:px-0 max-md:py-0";
const EMPTY = <span className='text-muted-foreground'>—</span>;

export function LocationTableRow({
  orgId,
  location,
  depth,
  locations
}: {
  orgId: string;
  location: LocationRow;
  depth: number;
  locations: LocationRow[];
}) {
  const externalIds = Object.entries(location.external_ids).sort(([a], [b]) => a.localeCompare(b));
  return (
    <TableRow data-testid={`settings-locations-row-${location.id}`}>
      <TableCell data-label='Name' className={CELL}>
        <div className='flex items-center gap-1.5' style={{ paddingLeft: `${depth * 1.25}rem` }}>
          {depth > 0 && (
            <CornerDownRight className='h-3.5 w-3.5 shrink-0 text-muted-foreground/60' />
          )}
          <div className='flex flex-col'>
            <span className='font-medium text-sm'>{location.name}</span>
            {location.external_id && (
              <span
                className='font-mono text-muted-foreground text-xs'
                data-testid={`settings-locations-external-id-${location.id}`}
              >
                {location.external_id}
              </span>
            )}
          </div>
        </div>
      </TableCell>
      <TableCell data-label='Kind' className={`${CELL} text-sm`}>
        {location.kind ?? EMPTY}
      </TableCell>
      <TableCell data-label='Status' className={CELL}>
        <LocationStatusBadge status={location.status} />
      </TableCell>
      <TableCell data-label='Timezone' className={`${CELL} text-muted-foreground text-xs`}>
        {location.timezone}
      </TableCell>
      <TableCell data-label='External ids' className={CELL}>
        {externalIds.length > 0 ? (
          <div className='flex flex-wrap gap-1'>
            {externalIds.map(([system, id]) => (
              <Badge
                key={system}
                variant='outline'
                className='font-mono font-normal text-muted-foreground text-xs'
              >
                {system}: {id}
              </Badge>
            ))}
          </div>
        ) : (
          EMPTY
        )}
      </TableCell>
      <TableCell className='w-12 px-2 py-3 text-right max-md:w-auto max-md:px-0 max-md:py-0'>
        <LocationRowActions orgId={orgId} location={location} locations={locations} />
      </TableCell>
    </TableRow>
  );
}
