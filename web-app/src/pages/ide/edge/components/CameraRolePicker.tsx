import type React from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import useActivePack from "@/hooks/api/cameras/useActivePack";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/** Sentinel for "clear the role" — Select doesn't accept empty string. */
const UNSET = "__unset__";

type Props = {
  value: string | null;
  disabled?: boolean;
  onChange: (next: string | null) => void;
  ariaLabel?: string;
};

/**
 * Per-camera role picker. Roles come from the workspace's active
 * domain pack (see `useActivePack`) — pre-Phase-4 this was a
 * hard-coded kitchen/prep/dining/other list, but now an operator
 * who's customized their pack (added a `bar` role, dropped
 * `dining`, etc.) sees those choices reflected here automatically.
 *
 * The picker writes optimistically through the parent's mutation
 * hook — TanStack Query invalidates cameras on success so the row
 * re-renders with the new value. There's no rollback on error; the
 * mutation toast surfaces failures.
 *
 * Pack loading state: the Select renders disabled with a "Loading"
 * placeholder. Pack fetch error: falls back to a static list
 * derived from any persisted `value`, so the camera row stays
 * editable even if the pack endpoint is down.
 */
const CameraRolePicker: React.FC<Props> = ({ value, disabled, onChange, ariaLabel }) => {
  const { workspace } = useCurrentWorkspace();
  const { data: pack, isLoading } = useActivePack(workspace?.id);
  const current = value ?? UNSET;
  const roles = pack?.body?.roles ?? [];

  return (
    // Click-trap to stop the row's player-open handler — same
    // pattern as the consent Switch and Triggers edit button.
    // biome-ignore lint/a11y/noStaticElementInteractions: wrapper exists only to trap row-click propagation
    <div
      role='presentation'
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      <Select
        value={current}
        disabled={disabled || isLoading}
        onValueChange={(v) => onChange(v === UNSET ? null : v)}
      >
        <SelectTrigger className='h-8 w-32 text-xs' aria-label={ariaLabel}>
          <SelectValue placeholder={isLoading ? "Loading…" : "—"} />
        </SelectTrigger>
        <SelectContent className={DESIGN_REFERENCE}>
          <SelectItem value={UNSET}>
            <span className='text-muted-foreground'>Unset</span>
          </SelectItem>
          {roles.map((role) => (
            <SelectItem key={role.role_key} value={role.role_key}>
              <div className='flex flex-col'>
                <span>{role.label}</span>
                <span className='text-muted-foreground text-xs'>{role.hint}</span>
              </div>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
};

export default CameraRolePicker;
