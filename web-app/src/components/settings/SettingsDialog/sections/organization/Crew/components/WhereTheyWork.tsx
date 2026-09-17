import { Plus, X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Label } from "@/components/ui/shadcn/label";
import { useLocations, useOrgRoles, usePeople } from "@/hooks/api/organizations";
import { LocationSelect } from "../../shared/LocationSelect";
import { NO_PERSON, PersonSelect } from "../../shared/PersonSelect";
import { PositionSelect } from "../../shared/PositionSelect";

/** One "where they work" row while the enrol form is open; `key` is only for React. */
export interface WorkDraft {
  key: number;
  role_id: string;
  location_id: string;
  supervisor_id: string;
}

/** Why the rows can't be sent, or null when they can. */
export function workDraftsProblem(rows: WorkDraft[]): string | null {
  for (const row of rows) {
    if (!row.role_id || !row.location_id) {
      return "Pick a position and a location for each row, or remove the row.";
    }
  }
  return null;
}

/**
 * Rows of position × location (+ who they report to) for a worker being
 * enrolled. Only location positions are offered: a crew member holds a
 * position at a place, and an org-wide one is assigned from Positions.
 */
export function WhereTheyWork({
  orgId,
  rows,
  onChange
}: {
  orgId: string;
  rows: WorkDraft[];
  onChange: (rows: WorkDraft[]) => void;
}) {
  const roles = useOrgRoles(orgId);
  const locations = useLocations(orgId);
  const { people } = usePeople(orgId);
  const nextKey = rows.reduce((max, row) => Math.max(max, row.key), -1) + 1;
  const update = (key: number, patch: Partial<WorkDraft>) =>
    onChange(rows.map((row) => (row.key === key ? { ...row, ...patch } : row)));

  return (
    <div className='space-y-1.5'>
      <Label>Where they work</Label>
      <div className='flex flex-col gap-2'>
        {rows.map((row, index) => (
          <div
            key={row.key}
            // `minmax(0,1fr)`, not `1fr`: a `1fr` track never shrinks below its
            // content's min width, so three selects showing "Reports to nobody"
            // pushed the row — and with it the whole form — past the dialog's
            // edge. Capped at 0, the tracks share the width and the triggers
            // truncate instead.
            className='grid gap-2 rounded-md border p-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]'
            data-testid={`settings-crew-enrol-work-row-${index}`}
          >
            <PositionSelect
              roles={roles.data ?? []}
              scope='location'
              value={row.role_id}
              onValueChange={(v) => update(row.key, { role_id: v })}
              placeholder='Position'
              testId={`settings-crew-enrol-work-position-${index}`}
            />
            <LocationSelect
              locations={locations.data ?? []}
              value={row.location_id}
              onValueChange={(v) => update(row.key, { location_id: v })}
              placeholder='Location'
              testId={`settings-crew-enrol-work-location-${index}`}
            />
            <PersonSelect
              people={people}
              value={row.supervisor_id}
              onValueChange={(v) => update(row.key, { supervisor_id: v })}
              allowNone
              noneLabel='Reports to nobody'
              placeholder='Reports to'
              testId={`settings-crew-enrol-work-supervisor-${index}`}
            />
            <Button
              type='button'
              variant='ghost'
              size='icon'
              className='h-9 w-9 shrink-0 text-muted-foreground'
              onClick={() => onChange(rows.filter((r) => r.key !== row.key))}
              aria-label='Remove this row'
              data-testid={`settings-crew-enrol-work-remove-${index}`}
            >
              <X className='h-4 w-4' />
            </Button>
          </div>
        ))}
        <Button
          type='button'
          variant='outline'
          size='sm'
          className='w-fit gap-1.5'
          onClick={() =>
            onChange([
              ...rows,
              { key: nextKey, role_id: "", location_id: "", supervisor_id: NO_PERSON }
            ])
          }
          data-testid='settings-crew-enrol-work-add'
        >
          <Plus className='h-3.5 w-3.5' />
          Add a place
        </Button>
      </div>
      <p className='text-muted-foreground text-xs'>
        Optional. A position says what they are called there; it grants no apps.
      </p>
    </div>
  );
}
