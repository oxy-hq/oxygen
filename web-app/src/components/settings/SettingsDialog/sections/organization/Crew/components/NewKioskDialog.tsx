import { useState } from "react";
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
import { useCreateDevice, useLocations } from "@/hooks/api/organizations";
import {
  DEFAULT_IDLE_TIMEOUT_SECONDS,
  IDLE_TIMEOUT_MAX_MINUTES,
  IDLE_TIMEOUT_MIN_MINUTES,
  idleTimeoutFromMinutes,
  idleTimeoutLabel
} from "@/libs/frontline";
import type { AppAccessSummary } from "@/types/appAccess";
import type { CreatedKioskDevice } from "@/types/frontline";
import { LocationSelect, NO_LOCATION } from "../../shared/LocationSelect";
import { apiErrorMessage, appReturnTo } from "../utils";

/** Radix Select can't carry an empty value, so "no app" needs a name. App ids are uuids. */
const ORG_HOME = "org-home";
/** Nothing chosen yet. Radix shows the placeholder for an empty value. */
const UNCHOSEN = "";

/**
 * What "Opens" starts on. A tablet exists to open an app — crew who land on
 * the org home are told the kiosk has no app to open — so the org's one app a
 * kiosk can open is preselected. With several there is no safe guess, so
 * nothing is, and Create waits for the admin. With none, the org home is all
 * there is. A draft doesn't count: its shell refuses crew until it's published.
 */
function defaultOpens(apps: AppAccessSummary[]): string {
  const openable = apps.filter((app) => app.published);
  if (openable.length === 1) return openable[0].id;
  return openable.length === 0 ? ORG_HOME : UNCHOSEN;
}

/** The line under "Opens": what the choice does for crew. */
function opensHint(appId: string, apps: AppAccessSummary[]): string {
  const app = apps.find((a) => a.id === appId);
  if (app) {
    return app.published
      ? `Crew land in ${app.name} as soon as they sign in.`
      : `${app.name} isn't published yet, so it won't open for crew until it is.`;
  }
  if (appId === UNCHOSEN) return "The app crew land in as soon as they sign in.";
  return apps.some((a) => a.published)
    ? "Crew sign in to a page with no app to open."
    : "No app is published to this organization yet, so crew sign in to a page with no app to open.";
}

export function NewKioskDialog({
  open,
  onOpenChange,
  orgId,
  orgSlug,
  apps,
  onCreated
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  orgSlug: string;
  apps: AppAccessSummary[];
  /** Called with the one-time enrol link; the caller must show it immediately. */
  onCreated: (device: CreatedKioskDevice) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-sm'>
        <DialogHeader>
          <DialogTitle>New kiosk</DialogTitle>
          <DialogDescription>
            A shared tablet crew sign in on. You'll get a link to open on it once.
          </DialogDescription>
        </DialogHeader>
        <NewKioskForm
          orgId={orgId}
          orgSlug={orgSlug}
          apps={apps}
          onCreated={(device) => {
            onOpenChange(false);
            onCreated(device);
          }}
          onCancel={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

function NewKioskForm({
  orgId,
  orgSlug,
  apps,
  onCreated,
  onCancel
}: {
  orgId: string;
  orgSlug: string;
  apps: AppAccessSummary[];
  onCreated: (device: CreatedKioskDevice) => void;
  onCancel: () => void;
}) {
  const createDevice = useCreateDevice();
  const locations = useLocations(orgId);
  const [name, setName] = useState("");
  // Derived until the admin picks, so an app list that answers after the
  // dialog opened still lands on its default instead of the org home.
  const [chosenAppId, setChosenAppId] = useState<string | null>(null);
  const appId = chosenAppId ?? defaultOpens(apps);
  const [locationId, setLocationId] = useState<string>(NO_LOCATION);
  const [idleMinutes, setIdleMinutes] = useState("");
  const [error, setError] = useState<string | null>(null);
  const idle = idleTimeoutFromMinutes(idleMinutes);
  const needsOpens = appId === UNCHOSEN;
  const canSubmit = name.trim().length > 0 && !needsOpens && idle.kind !== "invalid";

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canSubmit) return;
    setError(null);
    const app = apps.find((a) => a.id === appId);
    try {
      const device = await createDevice.mutateAsync({
        orgId,
        request: {
          name: name.trim(),
          ...(app ? { return_to: appReturnTo(orgSlug, app.slug) } : {}),
          ...(locationId !== NO_LOCATION ? { location_id: locationId } : {}),
          // Omitted on purpose when the box is empty: the column stays NULL and
          // the kiosk follows the platform default, rather than freezing
          // today's number into its own row.
          ...(idle.kind === "seconds" ? { idle_timeout_seconds: idle.seconds } : {})
        }
      });
      onCreated(device);
    } catch (err) {
      setError(apiErrorMessage(err, "Couldn't create the kiosk"));
    }
  };

  return (
    <form onSubmit={handleSubmit} className='flex flex-col gap-4 pt-1'>
      <div className='space-y-1.5'>
        <Label htmlFor='kiosk-name'>Name</Label>
        <Input
          id='kiosk-name'
          placeholder='Front counter'
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
          autoFocus
        />
      </div>
      <div className='space-y-1.5'>
        <Label htmlFor='kiosk-opens'>Opens</Label>
        <Select
          value={appId}
          // Never a pick: no item carries "". Inside a form, Radix mirrors the
          // value into a hidden native <select> and bubbles its `change`; when
          // the default moves to an app whose option hasn't registered yet,
          // that select reads "" and would wipe the default it just got.
          onValueChange={(value) => value !== UNCHOSEN && setChosenAppId(value)}
        >
          <SelectTrigger
            id='kiosk-opens'
            className='w-full'
            data-testid='settings-crew-kiosk-opens'
          >
            <SelectValue placeholder='Choose where the tablet lands' />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ORG_HOME}>Organization home</SelectItem>
            {apps.map((app) => (
              <SelectItem key={app.id} value={app.id}>
                {app.published ? app.name : `${app.name} (not published)`}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className='text-muted-foreground text-xs' data-testid='settings-crew-kiosk-opens-hint'>
          {opensHint(appId, apps)}
        </p>
      </div>
      <div className='space-y-1.5'>
        <Label htmlFor='kiosk-location'>Location</Label>
        <LocationSelect
          id='kiosk-location'
          locations={locations.data ?? []}
          value={locationId}
          onValueChange={setLocationId}
          allowNone
          noneLabel='No location'
          testId='settings-crew-kiosk-location'
        />
        <p className='text-muted-foreground text-xs'>The place this tablet sits at. Optional.</p>
      </div>
      <div className='space-y-1.5'>
        <Label htmlFor='kiosk-idle'>Signs itself out after</Label>
        <div className='flex items-center gap-2'>
          <Input
            id='kiosk-idle'
            type='number'
            inputMode='numeric'
            min={IDLE_TIMEOUT_MIN_MINUTES}
            max={IDLE_TIMEOUT_MAX_MINUTES}
            placeholder={String(DEFAULT_IDLE_TIMEOUT_SECONDS / 60)}
            value={idleMinutes}
            onChange={(e) => setIdleMinutes(e.target.value)}
            className='w-24'
            data-testid='settings-crew-kiosk-idle'
          />
          <span className='text-muted-foreground text-sm'>minutes</span>
        </div>
        <p
          className={
            idle.kind === "invalid" ? "text-destructive text-xs" : "text-muted-foreground text-xs"
          }
        >
          {idle.kind === "invalid"
            ? idle.message
            : `Idle time before the tablet signs the crew out. Empty is ${idleTimeoutLabel(DEFAULT_IDLE_TIMEOUT_SECONDS)}.`}
        </p>
      </div>
      {error && <p className='text-destructive text-sm'>{error}</p>}
      <div className='flex items-center justify-end gap-2'>
        {needsOpens && (
          <p
            id='kiosk-create-blocked'
            className='mr-auto text-muted-foreground text-xs'
            data-testid='settings-crew-new-kiosk-blocked'
          >
            Choose where the tablet lands first.
          </p>
        )}
        <Button type='button' variant='outline' size='sm' onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type='submit'
          size='sm'
          disabled={!canSubmit || createDevice.isPending}
          aria-describedby={needsOpens ? "kiosk-create-blocked" : undefined}
          data-testid='settings-crew-new-kiosk-submit'
        >
          {createDevice.isPending ? "Creating..." : "Create kiosk"}
        </Button>
      </div>
    </form>
  );
}
