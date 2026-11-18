import { useId, useState } from "react";
import { usePolicyPreview } from "@/hooks/api/airwayConfig/usePolicyPreview";
import {
  useDeleteAirwayGlobalConfig,
  useUpsertAirwayGlobalConfig
} from "@/hooks/api/airwayConfig/useUpsertAirwayConfig";
import {
  type AirwayContractPolicy,
  type AirwayEnvironment,
  type AirwaySourceKindConfig,
  asContractPolicy,
  asEnvironment
} from "@/services/api/airwayConfig";
import { SaveConfirmDialog } from "../components/SaveConfirmDialog";
import { unrecognisedFields } from "../summary";
import { computeSaveGate } from "../utils";
import { SourceKindEditor } from "./components/SourceKindEditor";
import { SourceKindLine } from "./components/SourceKindLine";

/**
 * One source kind in the `/admin/airway` table: a one-line summary of its stored
 * global row, and (expanded) the editor for it — the two selects, a lazy preview,
 * and its per-workspace overrides.
 *
 * Changing either select never saves by itself — it only arms Save and drops any
 * preview already on screen, so an operator can never save policy B while looking at
 * a preview of policy A. The drafts live here rather than in the editor, so
 * collapsing a row keeps an unsaved edit and the line still says "Unsaved" with a
 * Save beside it. Collapsing does close the preview: a preview the operator is not
 * looking at is not one (see `computeSaveGate`'s `never-previewed`).
 */
export function SourceKindRow({ kind }: { kind: AirwaySourceKindConfig }) {
  // Narrowed at the boundary: the column is free text (no CHECK — see the
  // migration), so a row written by raw SQL or by a build that knew a different
  // spelling arrives as a string outside the union. Seeding the select with it
  // renders a blank `SelectValue` and fails only at `PUT`, with a 400. An
  // unrecognised value therefore seeds "inherit" and is reported explicitly
  // (in the line, and in full in the editor) instead of a blank field.
  const serverPolicy = asContractPolicy(kind.global?.contract_policy);
  const serverEnv = asEnvironment(kind.global?.environment);
  const unrecognised = unrecognisedFields(kind.global);

  const [open, setOpen] = useState(false);
  const [draftPolicy, setDraftPolicy] = useState<AirwayContractPolicy | null>(serverPolicy);
  const [draftEnv, setDraftEnv] = useState<AirwayEnvironment | null>(serverEnv);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const editorId = useId();

  const dirty = draftPolicy !== serverPolicy || draftEnv !== serverEnv;

  // Lifted here (not inside PolicyPreview) so Save can gate its confirm step
  // on the exact same data the disclosure renders — no separate fetch, no
  // risk of the confirm reading a different policy than what's on screen.
  // BOTH admission axes are passed: `environment` is part of the query key and
  // of the request, so changing it is a cache miss rather than a silent reuse.
  const preview = usePolicyPreview(
    kind.source_kind,
    draftPolicy ?? undefined,
    draftEnv ?? undefined,
    { enabled: previewOpen }
  );
  const upsert = useUpsertAirwayGlobalConfig();
  const deleteGlobal = useDeleteAirwayGlobalConfig();

  // Never derive the gate from `preview.data` alone, never from
  // `preview.isSuccess` without `previewOpen`, and never without telling it
  // which draft is about to be saved — the gate verifies the body it trusts
  // was computed for that exact `(contract_policy, environment)`. See
  // `computeSaveGate`'s doc for the full list of what "unknown" covers.
  //
  // `"global"`: this row writes the kind's fleet-wide row, so a preview the
  // server fenced to this operator's platform scope answered only part of the
  // question. That is the one tier where a non-zero `out_of_scope_pipelines`
  // must confirm.
  const gate = computeSaveGate(
    preview,
    previewOpen,
    { contractPolicy: draftPolicy, environment: draftEnv },
    "global"
  );

  function toggle() {
    if (open) setPreviewOpen(false);
    setOpen(!open);
  }

  function doSave() {
    // Always both fields — a PUT is a replace, not a patch; omitting one
    // would silently wipe it back to "inherit".
    upsert.mutate(
      {
        sourceKind: kind.source_kind,
        body: { contract_policy: draftPolicy, environment: draftEnv }
      },
      { onSuccess: () => setConfirmOpen(false) }
    );
  }

  function handleSaveClick() {
    if (gate.kind === "clean") {
      doSave();
    } else {
      setConfirmOpen(true);
    }
  }

  // "Clear" removes the global row entirely (distinct from saving both
  // fields as `null`, which still leaves a row behind) — the un-set
  // direction, so it skips the confirm gate the same way an un-dirtying
  // edit would. Resets the drafts too, or `dirty` would immediately read
  // `true` again against the now-`null` server row.
  function handleClearClick() {
    deleteGlobal.mutate(kind.source_kind, {
      onSuccess: () => {
        setDraftPolicy(null);
        setDraftEnv(null);
        setPreviewOpen(false);
      }
    });
  }

  return (
    <li
      className='col-span-full grid grid-cols-subgrid border-border/50 border-b last:border-b-0'
      data-testid={`admin-airway-card-${kind.source_kind}`}
      data-state={open ? "open" : "closed"}
    >
      <SourceKindLine
        kind={kind}
        open={open}
        onToggle={toggle}
        editorId={editorId}
        dirty={dirty}
        saving={upsert.isPending}
        onSave={handleSaveClick}
      />

      {open ? (
        <SourceKindEditor
          id={editorId}
          kind={kind}
          unrecognised={unrecognised}
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
          clearing={deleteGlobal.isPending}
          clearDisabled={upsert.isPending || deleteGlobal.isPending}
          onClear={handleClearClick}
        />
      ) : null}

      <SaveConfirmDialog
        testIdSuffix={kind.source_kind}
        open={confirmOpen}
        pending={upsert.isPending}
        gate={gate}
        onOpenChange={setConfirmOpen}
        onConfirm={doSave}
        // From a collapsed row too: the fix for "unknown impact" is to look, so
        // open the row as well as the preview.
        onPreviewInstead={() => {
          setOpen(true);
          setPreviewOpen(true);
        }}
      />
    </li>
  );
}
