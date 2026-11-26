import type { UseQueryResult } from "@tanstack/react-query";
import { RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type {
  AirwayContractPolicy,
  AirwayEnvironment,
  AirwayPolicyPreviewResponse,
  AirwaySourceKindConfig
} from "@/services/api/airwayConfig";
import { PolicyFields } from "../../components/PolicyFields";
import { PolicyPreview, PolicyPreviewTrigger } from "../../PolicyPreview";
import { WorkspaceOverrides } from "../../WorkspaceOverrides";

/**
 * The expanded half of a source-kind row: the two selects with the row's actions
 * beside them, the preview's results when asked for, and the kind's workspace
 * overrides. Holds no state: `SourceKindRow` owns the drafts and the preview so the
 * collapsed line can still show and save an unsaved edit.
 */
export function SourceKindEditor({
  id,
  kind,
  unrecognised,
  draftPolicy,
  draftEnv,
  onPolicyChange,
  onEnvChange,
  previewOpen,
  onRequestPreview,
  onHidePreview,
  preview,
  clearing,
  clearDisabled,
  onClear
}: {
  id: string;
  kind: AirwaySourceKindConfig;
  /** `unrecognisedFields(kind.global)`, computed once by the row. */
  unrecognised: string[];
  draftPolicy: AirwayContractPolicy | null;
  draftEnv: AirwayEnvironment | null;
  onPolicyChange: (value: AirwayContractPolicy | null) => void;
  onEnvChange: (value: AirwayEnvironment | null) => void;
  previewOpen: boolean;
  onRequestPreview: () => void;
  onHidePreview: () => void;
  preview: UseQueryResult<AirwayPolicyPreviewResponse>;
  clearing: boolean;
  clearDisabled: boolean;
  onClear: () => void;
}) {
  const sourceKind = kind.source_kind;

  return (
    // `pl` lines the editor up under the kind's name, past the row's chevron.
    <div
      id={id}
      className='col-span-full space-y-4 border-border/50 border-t bg-muted/20 px-3 py-3 sm:pl-[1.875rem]'
      data-testid={`admin-airway-editor-${sourceKind}`}
    >
      {unrecognised.length > 0 && (
        <p
          className=' border border-status-warning-text/40 bg-status-warning-bg px-3 py-2 text-status-warning-text text-xs'
          data-testid={`admin-airway-unrecognised-${sourceKind}`}
        >
          This kind's stored row holds a value this build does not recognise (
          {unrecognised.join(", ")}). Airway refuses it at run time rather than falling back, so
          these pipelines are already failing. The fields below show "inherit"; saving replaces the
          stored value.
        </p>
      )}

      <div className='flex flex-wrap items-start gap-x-4 gap-y-2'>
        <PolicyFields
          className='min-w-0 max-w-xl flex-1'
          idSuffix={sourceKind}
          draftPolicy={draftPolicy}
          draftEnv={draftEnv}
          onPolicyChange={onPolicyChange}
          onEnvChange={onEnvChange}
        />
        {/* Offset by the fields' label so the buttons sit level with the selects. */}
        <div className='flex items-center gap-1.5 sm:mt-6'>
          {previewOpen ? null : (
            <PolicyPreviewTrigger sourceKind={sourceKind} onRequestPreview={onRequestPreview} />
          )}
          {kind.global !== null && (
            <Button
              type='button'
              variant='ghost'
              size='sm'
              disabled={clearDisabled}
              onClick={onClear}
              data-testid={`admin-airway-clear-${sourceKind}`}
              tooltip="Remove this kind's global row — every workspace falls back to airway's built-in default"
            >
              {clearing ? <Spinner className='size-3.5' /> : <RotateCcw className='size-3.5' />}
              Clear
            </Button>
          )}
        </div>
      </div>

      {previewOpen ? (
        <PolicyPreview
          sourceKind={sourceKind}
          open
          onRequestPreview={onRequestPreview}
          onHide={onHidePreview}
          preview={preview}
        />
      ) : null}

      <WorkspaceOverrides sourceKind={sourceKind} overrides={kind.overrides} global={kind.global} />
    </div>
  );
}
