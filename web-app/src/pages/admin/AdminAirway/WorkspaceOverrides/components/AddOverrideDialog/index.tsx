import { useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { usePolicyPreview } from "@/hooks/api/airwayConfig/usePolicyPreview";
import { useUpsertAirwayWorkspaceOverride } from "@/hooks/api/airwayConfig/useUpsertAirwayConfig";
import { SaveConfirmDialog } from "@/pages/admin/AdminAirway/components/SaveConfirmDialog";
import { computeSaveGate, resolveInherited } from "@/pages/admin/AdminAirway/utils";
import type {
  AirwayConfigValues,
  AirwayContractPolicy,
  AirwayEnvironment
} from "@/services/api/airwayConfig";
import { AddOverrideForm } from "./components/AddOverrideForm";

interface AddOverrideDialogProps {
  sourceKind: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Workspaces that already have an override for this kind — kept out of the picker. */
  existingWorkspaceIds: string[];
  /**
   * The kind's global row — what a field left on "inherit" actually resolves
   * to. Required for the preview to describe this save; see `resolveInherited`.
   */
  global: AirwayConfigValues | null;
}

/**
 * Creates one workspace override — the write half of the escape hatch this
 * whole stage exists to build. An override is deliberately *sparse*: an
 * operator sets only the one field they mean to diverge on and leaves the
 * other as "inherit", so this reuses the same `PolicyFields` the global row
 * uses (its `null` == inherit is exactly that mechanism) rather than forcing
 * both.
 *
 * Guarded the same way as the global row: creating an override can halt
 * that workspace's pipelines just as surely as editing the kind's global
 * policy can, so this embeds the same lazy preview + `computeSaveGate` +
 * `SaveConfirmDialog` rather than saving unconditionally (see
 * `components/AddOverrideForm.tsx` for the note on why the embedded preview
 * is kind-level, not workspace-scoped).
 */
export function AddOverrideDialog({
  sourceKind,
  open,
  onOpenChange,
  existingWorkspaceIds,
  global
}: AddOverrideDialogProps) {
  const [workspace, setWorkspace] = useState<{ id: string; name: string } | null>(null);
  const [draftPolicy, setDraftPolicy] = useState<AirwayContractPolicy | null>(null);
  const [draftEnv, setDraftEnv] = useState<AirwayEnvironment | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);

  // What this override will ACTUALLY run under — `draft ?? global ?? airway's
  // default`, mirroring `resolve_admission`'s field-by-field merge. A field
  // left on "inherit" here inherits the kind's GLOBAL ROW, not the built-in
  // default, so previewing the raw draft asks the server a different question:
  // it scores `permissive`, the echo matches a subject that also defaulted to
  // `permissive`, and the gate reads clean for a policy nobody scored.
  const effectivePolicy = resolveInherited(draftPolicy, global?.contract_policy);
  const effectiveEnv = resolveInherited(draftEnv, global?.environment);

  // Both admission axes, same as the global row: `environment` rides the query
  // key and the request so an `environment`-only change cannot reuse a cached
  // scan that never considered it.
  const preview = usePolicyPreview(
    sourceKind,
    effectivePolicy ?? undefined,
    effectiveEnv ?? undefined,
    { enabled: previewOpen }
  );
  // See `computeSaveGate`'s doc: `previewOpen` must gate this, and the RESOLVED
  // settings must be passed so the gate can verify the body it trusts was
  // computed for what this save will run under — not for the sparse draft.
  //
  // `"override"`: this write lands on exactly one workspace, and the override
  // routes already fence that workspace to the caller's platform scope — so the
  // scan covered every pipeline this save can touch, and the fleet-wide
  // remainder it reports is not a gap in *this* answer. Passing `"global"` here
  // would confirm every override an in-scope operator ever adds.
  const gate = computeSaveGate(
    preview,
    previewOpen,
    { contractPolicy: effectivePolicy, environment: effectiveEnv },
    "override"
  );
  const upsert = useUpsertAirwayWorkspaceOverride();

  function reset() {
    setWorkspace(null);
    setDraftPolicy(null);
    setDraftEnv(null);
    setPreviewOpen(false);
    setConfirmOpen(false);
  }

  function handleOpenChange(next: boolean) {
    if (!next && !upsert.isPending) reset();
    onOpenChange(next);
  }

  function doSubmit() {
    if (!workspace) return;
    // Always both fields — same replace-not-patch semantics as the global row.
    upsert.mutate(
      {
        sourceKind,
        workspaceId: workspace.id,
        body: { contract_policy: draftPolicy, environment: draftEnv }
      },
      { onSuccess: () => handleOpenChange(false) }
    );
  }

  const canSubmit = workspace !== null && (draftPolicy !== null || draftEnv !== null);

  function handleAddClick() {
    if (!canSubmit) return;
    if (gate.kind === "clean") {
      doSubmit();
    } else {
      setConfirmOpen(true);
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className={DESIGN_REFERENCE}
        data-testid={`admin-airway-add-override-dialog-${sourceKind}`}
      >
        <DialogHeader>
          <DialogTitle>Add a workspace override for {sourceKind}</DialogTitle>
          <DialogDescription>
            Set only the field this workspace needs to diverge on — leave the other as "inherit" to
            keep following the kind's global policy above.
          </DialogDescription>
        </DialogHeader>

        <AddOverrideForm
          sourceKind={sourceKind}
          workspace={workspace}
          onWorkspaceChange={setWorkspace}
          excludeWorkspaceIds={existingWorkspaceIds}
          draftPolicy={draftPolicy}
          draftEnv={draftEnv}
          onPolicyChange={(value) => {
            setDraftPolicy(value);
            setPreviewOpen(false);
          }}
          onEnvChange={(value) => {
            setDraftEnv(value);
            setPreviewOpen(false);
          }}
          previewOpen={previewOpen}
          onRequestPreview={() => setPreviewOpen(true)}
          onHidePreview={() => setPreviewOpen(false)}
          preview={preview}
        />

        <DialogFooter>
          <Button type='button' variant='outline' onClick={() => handleOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type='button'
            disabled={!canSubmit || upsert.isPending}
            onClick={handleAddClick}
            data-testid={`admin-airway-add-override-submit-${sourceKind}`}
          >
            {upsert.isPending && <Spinner className='size-3.5' />}
            Add override
          </Button>
        </DialogFooter>

        <SaveConfirmDialog
          testIdSuffix={`add-override-${sourceKind}`}
          open={confirmOpen}
          pending={upsert.isPending}
          gate={gate}
          onOpenChange={setConfirmOpen}
          onConfirm={doSubmit}
          onPreviewInstead={() => setPreviewOpen(true)}
        />
      </DialogContent>
    </Dialog>
  );
}
