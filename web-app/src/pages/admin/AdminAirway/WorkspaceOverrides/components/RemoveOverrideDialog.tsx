import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import type { AirwayConfigValues, AirwayWorkspaceOverride } from "@/services/api/airwayConfig";

interface RemoveOverrideDialogProps {
  sourceKind: string;
  /** The row awaiting confirmation; `null` closes the dialog. */
  override: AirwayWorkspaceOverride | null;
  /** This kind's global row — what the workspace falls back to. `null` = no row, so airway's own defaults. */
  global: AirwayConfigValues | null;
  pending: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

/** Airway's built-in defaults, which apply when no global row exists (or the field is unset on it). */
const AIRWAY_DEFAULT_POLICY = "permissive";
const AIRWAY_DEFAULT_ENVIRONMENT = "production";

/**
 * What one field resolves to once the override is gone: the global row's
 * value, or airway's own default when the row doesn't set it. Named so the
 * operator can tell "the admin set this" from "nobody set anything".
 */
function fallbackLabel(value: string | null | undefined, airwayDefault: string): string {
  return value ?? `${airwayDefault} (airway default)`;
}

/**
 * Confirms removing a per-workspace override.
 *
 * Removing an override is a **policy change on that workspace**, not a
 * cleanup: it falls back to the kind's global row, which may be *stricter*
 * than what the override set. That is the same outage the Save confirmation
 * exists to prevent — pipelines whose resources don't satisfy the tightened
 * policy stop at their next run, and the refusal only surfaces as a config
 * error from a queued worker — reached by a different button.
 *
 * Deliberately **not** the same component as `SaveConfirmDialog`. That one
 * grades an impact it has actually measured (a preview of this kind across
 * every workspace); this one cannot, because the preview endpoint takes no
 * workspace and so cannot score "this workspace under the global row". So it
 * states the fallback concretely — which policy and environment take over —
 * and lets the operator judge, rather than borrowing a confidence it doesn't
 * have.
 */
export function RemoveOverrideDialog({
  sourceKind,
  override,
  global,
  pending,
  onOpenChange,
  onConfirm
}: RemoveOverrideDialogProps) {
  const name = override?.workspace_name ?? override?.workspace_id ?? "";

  return (
    <AlertDialog
      open={override !== null}
      onOpenChange={(next) => {
        if (!next && !pending) onOpenChange(false);
      }}
    >
      <AlertDialogContent
        className={DESIGN_REFERENCE}
        data-testid={`admin-airway-remove-override-dialog-${sourceKind}`}
      >
        <AlertDialogHeader>
          <AlertDialogTitle>Remove this workspace override?</AlertDialogTitle>
          <AlertDialogDescription>
            <span className='font-medium'>{name}</span> stops overriding {sourceKind} and falls back
            to this kind's global row: contract policy{" "}
            <span className='font-mono'>
              {fallbackLabel(global?.contract_policy, AIRWAY_DEFAULT_POLICY)}
            </span>
            , environment{" "}
            <span className='font-mono'>
              {fallbackLabel(global?.environment, AIRWAY_DEFAULT_ENVIRONMENT)}
            </span>
            . If that is stricter than the override being removed, this workspace's pipelines halt
            at their next run — the same failure the Save confirmation guards against, reached from
            here instead.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={pending}
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
            data-testid={`admin-airway-remove-override-confirm-${sourceKind}`}
          >
            {pending ? "Removing…" : "Remove override"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
