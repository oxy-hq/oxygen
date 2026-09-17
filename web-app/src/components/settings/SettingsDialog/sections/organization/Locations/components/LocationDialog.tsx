import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import {
  useCreateLocation,
  useDeleteExternalId,
  useSetExternalId,
  useUpdateLocation
} from "@/hooks/api/organizations";
import { apiErrorMessage, apiStatus } from "@/libs/apiError";
import type { LocationRow, LocationStatus } from "@/types/operatingGraph";
import { LocationSelect, NO_LOCATION } from "../../shared/LocationSelect";
import {
  browserTimeZone,
  descendantIds,
  draftsToRecord,
  type ExternalIdDraft,
  externalIdDiff,
  externalIdsProblem,
  LOCATION_STATUS_LABELS,
  LOCATION_STATUSES,
  locationPatch,
  recordToDrafts,
  usedKinds
} from "../utils";
import { ExternalIdsEditor } from "./ExternalIdsEditor";
import { TimezoneSelect } from "./TimezoneSelect";

/**
 * One dialog for both New location and Edit location: the fields are the
 * same, and a create that lands but whose external ids are refused turns
 * into an edit of the row that now exists — so a retry never creates twice.
 */
export function LocationDialog({
  open,
  onOpenChange,
  orgId,
  locations,
  location
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  locations: LocationRow[];
  /** Omit to create. */
  location?: LocationRow;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-lg'>
        <DialogHeader>
          <DialogTitle>{location ? `Edit ${location.name}` : "New location"}</DialogTitle>
          <DialogDescription>
            {location
              ? "Change what this place is called, where it sits and how other systems know it."
              : "A place work happens: a region, a store, a station. You name the levels."}
          </DialogDescription>
        </DialogHeader>
        {/* Mounted fresh on every open, so the form seeds from the row as it
            is now and never from an edit abandoned last time. */}
        <LocationForm
          orgId={orgId}
          locations={locations}
          location={location}
          onDone={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

interface ExternalIdError {
  system: string;
  message: string;
}

function LocationForm({
  orgId,
  locations,
  location,
  onDone
}: {
  orgId: string;
  locations: LocationRow[];
  location?: LocationRow;
  onDone: () => void;
}) {
  const create = useCreateLocation();
  const update = useUpdateLocation();
  const setExternalId = useSetExternalId();
  const deleteExternalId = useDeleteExternalId();

  // The row this form edits. Starts as the prop; a successful create fills
  // it, so from then on submit patches rather than posts again.
  const [existing, setExisting] = useState<LocationRow | null>(location ?? null);
  const [name, setName] = useState(location?.name ?? "");
  const [kind, setKind] = useState(location?.kind ?? "");
  const [parentId, setParentId] = useState(location?.parent_id ?? NO_LOCATION);
  const [status, setStatus] = useState<LocationStatus>(location?.status ?? "pre_launch");
  const [timezone, setTimezone] = useState(location?.timezone ?? browserTimeZone());
  const [placeId, setPlaceId] = useState(location?.external_id ?? "");
  const [externalIds, setExternalIds] = useState<ExternalIdDraft[]>(
    recordToDrafts(location?.external_ids ?? {})
  );
  const [formError, setFormError] = useState<string | null>(null);
  const [externalIdError, setExternalIdError] = useState<ExternalIdError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const kinds = usedKinds(locations);
  const excluded = existing ? descendantIds(locations, existing.id) : undefined;
  const isPending =
    create.isPending || update.isPending || setExternalId.isPending || deleteExternalId.isPending;
  const canSubmit = name.trim().length > 0 && timezone.length > 0;

  /** PUT what changed, DELETE what went, one at a time, keeping `existing` honest as we go. */
  const applyExternalIds = async (row: LocationRow) => {
    // A server older than the graph answers a bare row with no `external_ids`;
    // treat that as "none yet" rather than a failed create.
    const before = row.external_ids ?? {};
    const { set, remove } = externalIdDiff(before, draftsToRecord(externalIds));
    const current = { ...before };
    try {
      for (const [system, externalId] of set) {
        await setExternalId.mutateAsync({ orgId, locationId: row.id, system, externalId });
        current[system] = externalId;
      }
      for (const system of remove) {
        await deleteExternalId.mutateAsync({ orgId, locationId: row.id, system });
        delete current[system];
      }
    } catch (err) {
      setExisting({ ...row, external_ids: current });
      const failed = set.find(
        ([system]) => !(system in current) || current[system] !== before[system]
      );
      const system = failed?.[0] ?? remove.find((s) => s in current) ?? "";
      const message =
        apiStatus(err) === 409
          ? "Another location already carries that id."
          : apiErrorMessage(err, "Couldn't save that external id");
      setExternalIdError({ system, message });
      return false;
    }
    return true;
  };

  const draft = () => ({
    name,
    kind,
    parentId: parentId === NO_LOCATION ? null : parentId,
    status,
    timezone,
    externalId: placeId
  });

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSubmit || isPending) return;
    setFormError(null);
    setExternalIdError(null);
    setNote(null);
    const problem = externalIdsProblem(externalIds);
    if (problem) {
      setExternalIdError({ system: "", message: problem });
      return;
    }

    try {
      let row: LocationRow;
      let verb: string;
      if (existing) {
        const patch = locationPatch(existing, draft());
        row =
          Object.keys(patch).length > 0
            ? await update.mutateAsync({ orgId, locationId: existing.id, request: patch })
            : existing;
        verb = "Saved";
      } else {
        row = await create.mutateAsync({
          orgId,
          request: {
            name: name.trim(),
            ...(kind.trim() ? { kind: kind.trim() } : {}),
            ...(parentId !== NO_LOCATION ? { parent_id: parentId } : {}),
            ...(placeId.trim() ? { external_id: placeId.trim() } : {}),
            status,
            timezone
          }
        });
        setExisting(row);
        verb = "Created";
      }
      if (!(await applyExternalIds(row))) {
        if (verb === "Created") {
          setNote(`Created ${row.name}. Fix the external id below and save again.`);
        }
        return;
      }
      toast.success(`${verb} ${row.name}`);
      onDone();
    } catch (err) {
      setFormError(
        apiErrorMessage(
          err,
          existing ? "Couldn't save the location" : "Couldn't create the location"
        )
      );
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex flex-col gap-4 pt-1'>
      <div className='grid gap-4 sm:grid-cols-2'>
        <div className='space-y-1.5'>
          <Label htmlFor='location-name'>Name</Label>
          <Input
            id='location-name'
            placeholder='Clovis'
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            autoFocus
            data-testid='settings-locations-name'
          />
        </div>
        <div className='space-y-1.5'>
          <Label htmlFor='location-kind'>Kind</Label>
          <Input
            id='location-kind'
            list='location-kinds'
            placeholder='store, region, station'
            autoComplete='off'
            value={kind}
            onChange={(e) => setKind(e.target.value)}
            data-testid='settings-locations-kind'
          />
          <datalist id='location-kinds'>
            {kinds.map((k) => (
              <option key={k} value={k} />
            ))}
          </datalist>
        </div>
      </div>
      <div className='space-y-1.5'>
        <Label htmlFor='location-parent'>Parent</Label>
        <LocationSelect
          id='location-parent'
          locations={locations}
          value={parentId}
          onValueChange={setParentId}
          allowNone
          noneLabel='None (top level)'
          includeClosed
          exclude={excluded}
          testId='settings-locations-parent'
        />
      </div>
      <div className='grid gap-4 sm:grid-cols-2'>
        <div className='space-y-1.5'>
          <Label htmlFor='location-status'>Status</Label>
          <Select value={status} onValueChange={(v) => setStatus(v as LocationStatus)}>
            <SelectTrigger
              id='location-status'
              className='w-full'
              data-testid='settings-locations-status'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {LOCATION_STATUSES.map((s) => (
                <SelectItem key={s} value={s}>
                  {LOCATION_STATUS_LABELS[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className='space-y-1.5'>
          <Label>Timezone</Label>
          <TimezoneSelect
            value={timezone}
            onValueChange={setTimezone}
            testId='settings-locations-timezone'
          />
        </div>
      </div>
      <div className='space-y-1.5'>
        <Label htmlFor='location-external-id'>Place id</Label>
        <Input
          id='location-external-id'
          className='font-mono'
          placeholder='santa-clara'
          autoComplete='off'
          value={placeId}
          onChange={(e) => setPlaceId(e.target.value)}
          data-testid='settings-locations-external-id'
        />
        <p className='text-muted-foreground text-xs'>
          Your own id for this place — what an app keys it by, sent as external_id. Unique in this
          org. Ids other systems use go under External ids.
        </p>
      </div>
      <div className='space-y-1.5'>
        <Label>External ids</Label>
        <ExternalIdsEditor
          rows={externalIds}
          onChange={(rows) => {
            setExternalIds(rows);
            if (externalIdError) setExternalIdError(null);
          }}
          errorSystem={externalIdError?.system}
        />
        {externalIdError ? (
          <p
            className='text-destructive text-sm'
            data-testid='settings-locations-external-id-error'
          >
            {externalIdError.message}
          </p>
        ) : (
          <p className='text-muted-foreground text-xs'>
            How other systems know this place, like toast: 1234. Each id is unique within a system.
          </p>
        )}
      </div>
      {note && <p className='text-muted-foreground text-sm'>{note}</p>}
      {formError && <p className='text-destructive text-sm'>{formError}</p>}
      <div className='flex justify-end gap-2'>
        <Button type='button' variant='outline' size='sm' onClick={onDone}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={!canSubmit || isPending}
          data-testid='settings-locations-submit'
        >
          {isPending ? "Saving..." : existing ? "Save changes" : "Create location"}
        </Button>
      </div>
    </form>
  );
}
