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
import { Button } from "@/components/ui/shadcn/button";
import type { SaveGate } from "../utils";

interface SaveConfirmDialogProps {
  /** Distinguishes testids across the global row and per-kind add-override dialogs. */
  testIdSuffix: string;
  open: boolean;
  pending: boolean;
  /** Never `{ kind: "clean" }` in practice — the caller saves directly for that case and never opens this dialog. */
  gate: SaveGate;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  /** Closes this dialog and opens the preview panel instead — only offered for the "unknown impact" gate, where previewing is the actual fix. */
  onPreviewInstead?: () => void;
}

/** The reason clause for an "unknown" gate — a sentence fragment ending right before ", so...". */
function describeUnknownReason(gate: Extract<SaveGate, { kind: "unknown" }>): string {
  switch (gate.reason) {
    case "never-previewed":
      return "This policy hasn't been previewed";
    case "loading":
      return "The preview for this policy is still loading";
    case "error":
      return "The preview for this policy failed to load";
    case "incomplete": {
      const n = gate.unevaluatedCount ?? 0;
      return `Coverage is incomplete — ${n} pipeline${n === 1 ? "" : "s"} could not be evaluated`;
    }
    case "partial-scope": {
      // Says what the scan could not see, and why that matters *here*: this
      // reason only ever reaches the global row, whose reach is the whole
      // fleet while the preview's reach was this operator's platform scope.
      const n = gate.outOfScopeCount ?? 0;
      return `This global policy applies to every workspace, and the preview could not see all of them — ${n} compiled pipeline${
        n === 1 ? "" : "s"
      } in workspaces outside your platform scope went unscanned`;
    }
    case "stale":
      return "The preview on screen was computed for different settings than the ones being saved";
  }
}

/**
 * Gates every save that isn't a *known-clean* preview. A known set of
 * failures gets the original "N resources would fail" copy; anything else
 * (never previewed / disclosure closed, still loading, errored, a clean scan
 * with incomplete `unevaluated` coverage, or — for the fleet-wide global row —
 * a clean scan of only the tenants the operator's platform scope reaches) is
 * treated as an **unknown** blast radius, not a safe one — that gap is exactly
 * what let saves through with no confirmation at all before this component
 * existed.
 */
export function SaveConfirmDialog({
  testIdSuffix,
  open,
  pending,
  gate,
  onOpenChange,
  onConfirm,
  onPreviewInstead
}: SaveConfirmDialogProps) {
  const isUnknown = gate.kind === "unknown";
  const failingCount = gate.kind === "failures" ? gate.failingCount : 0;

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (!next && !pending) onOpenChange(false);
      }}
    >
      <AlertDialogContent data-testid={`admin-airway-confirm-dialog-${testIdSuffix}`}>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {isUnknown
              ? "Save without knowing the impact?"
              : "Save a policy with failing resources?"}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {gate.kind === "unknown"
              ? `${describeUnknownReason(gate)}, so its impact on existing pipelines is unknown — not confirmed safe. Saving now may silently halt pipelines that don't satisfy it.`
              : `The preview for this policy shows ${failingCount} resource${
                  failingCount === 1 ? "" : "s"
                } that would fail admission. Saving halts every pipeline behind them until the resource is fixed or the policy is loosened again.`}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          {isUnknown && onPreviewInstead && (
            <Button
              type='button'
              variant='outline'
              disabled={pending}
              onClick={() => {
                onOpenChange(false);
                onPreviewInstead();
              }}
              data-testid={`admin-airway-confirm-preview-instead-${testIdSuffix}`}
            >
              Preview first
            </Button>
          )}
          <AlertDialogAction
            disabled={pending}
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
            data-testid={`admin-airway-confirm-save-${testIdSuffix}`}
          >
            {pending ? "Saving…" : "Save anyway"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
