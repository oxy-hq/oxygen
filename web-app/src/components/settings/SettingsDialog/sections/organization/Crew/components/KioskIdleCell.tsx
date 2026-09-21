import { Check, Loader2, X } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { useUpdateDevice } from "@/hooks/api/organizations";
import {
  DEFAULT_IDLE_TIMEOUT_SECONDS,
  IDLE_TIMEOUT_MAX_MINUTES,
  IDLE_TIMEOUT_MIN_MINUTES,
  idleTimeoutFromMinutes,
  idleTimeoutLabel,
  idleTimeoutMinutesField,
  idleTimeoutPatch
} from "@/libs/frontline";
import type { KioskDeviceRow } from "@/types/frontline";
import { apiErrorMessage } from "../utils";

/**
 * The **Signs out** column, editable in place.
 *
 * Until `PATCH …/frontline/devices/{id}` existed this was a read-only number
 * and changing it meant revoking a counter tablet and walking a new enrol link
 * out to it — which is the wrong answer for a store that wants to tune the
 * timeout after a shift. The tablet keeps its cookie through this.
 *
 * The box is the enrol dialog's box: whole minutes, empty means the platform
 * default, and the same refusal sentence before the server's 400 rather than
 * after it. Empty is not "unchanged" — saving it sends `null`, which is what
 * puts a kiosk back on the default (see `idleTimeoutPatch`).
 *
 * A revoked kiosk is not editable: the server refuses it with a 409, because
 * that row is the record of which tablet a shift was signed in on.
 */
export function KioskIdleCell({
  orgId,
  device,
  editable
}: {
  orgId: string;
  device: KioskDeviceRow;
  /** False for a revoked kiosk, whose row the server will not let anyone change. */
  editable: boolean;
}) {
  const [editing, setEditing] = useState(false);

  if (!editing) {
    const label = idleTimeoutLabel(device.idle_timeout_seconds);
    return editable ? (
      <Button
        variant='link'
        size='sm'
        className='h-auto p-0 font-normal text-foreground text-sm'
        onClick={() => setEditing(true)}
        aria-label={`Change when ${device.name} signs out`}
        data-testid={`settings-crew-kiosk-idle-edit-${device.id}`}
      >
        {label}
      </Button>
    ) : (
      <span className='text-sm'>{label}</span>
    );
  }

  // No `key` off the row on purpose: the device list polls while any enrol link
  // is live, and remounting on a refetch would wipe what the admin is typing.
  return <IdleEditor orgId={orgId} device={device} onDone={() => setEditing(false)} />;
}

/**
 * Split out so the box resets from the row every time editing opens — the
 * component is mounted on entry and unmounted on Save or Cancel, which is
 * cheaper and harder to get wrong than syncing state to a prop.
 */
function IdleEditor({
  orgId,
  device,
  onDone
}: {
  orgId: string;
  device: KioskDeviceRow;
  onDone: () => void;
}) {
  const updateDevice = useUpdateDevice();
  const [minutes, setMinutes] = useState(() =>
    idleTimeoutMinutesField(device.idle_timeout_seconds)
  );
  const [error, setError] = useState<string | null>(null);
  const idle = idleTimeoutFromMinutes(minutes);
  const patch = idleTimeoutPatch(idle);

  const save = async () => {
    if (!patch) return;
    setError(null);
    try {
      await updateDevice.mutateAsync({ orgId, deviceId: device.id, request: patch });
      onDone();
    } catch (err) {
      // Inline, not a toast: the refusal is about the number in this box, and
      // it belongs beside the box the admin is still looking at.
      setError(apiErrorMessage(err, "Couldn't change when this kiosk signs out"));
    }
  };

  return (
    <div className='flex flex-col gap-1'>
      <div className='flex items-center gap-1'>
        <Input
          type='number'
          inputMode='numeric'
          min={IDLE_TIMEOUT_MIN_MINUTES}
          max={IDLE_TIMEOUT_MAX_MINUTES}
          placeholder={String(DEFAULT_IDLE_TIMEOUT_SECONDS / 60)}
          value={minutes}
          onChange={(e) => setMinutes(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void save();
            }
            if (e.key === "Escape") onDone();
          }}
          className='h-8 w-20'
          aria-label={`Minutes before ${device.name} signs out`}
          data-testid={`settings-crew-kiosk-idle-input-${device.id}`}
          // The box replaces the value the admin just clicked, as the name
          // field in `New kiosk` does when that dialog opens.
          autoFocus
        />
        <span className='text-muted-foreground text-xs'>min</span>
        <Button
          variant='ghost'
          size='icon'
          className='h-8 w-8'
          onClick={save}
          disabled={!patch || updateDevice.isPending}
          title='Save'
          aria-label={`Save when ${device.name} signs out`}
          data-testid={`settings-crew-kiosk-idle-save-${device.id}`}
        >
          {updateDevice.isPending ? (
            <Loader2 className='h-4 w-4 animate-spin' />
          ) : (
            <Check className='h-4 w-4' />
          )}
        </Button>
        <Button
          variant='ghost'
          size='icon'
          className='h-8 w-8 text-muted-foreground'
          onClick={onDone}
          disabled={updateDevice.isPending}
          title='Cancel'
          aria-label='Cancel'
          data-testid={`settings-crew-kiosk-idle-cancel-${device.id}`}
        >
          <X className='h-4 w-4' />
        </Button>
      </div>
      <p
        className={
          idle.kind === "invalid" || error
            ? "max-w-56 text-destructive text-xs"
            : "max-w-56 text-muted-foreground text-xs"
        }
        data-testid={`settings-crew-kiosk-idle-hint-${device.id}`}
      >
        {idle.kind === "invalid"
          ? idle.message
          : (error ?? `Empty is ${idleTimeoutLabel(DEFAULT_IDLE_TIMEOUT_SECONDS)}.`)}
      </p>
    </div>
  );
}
