import { AlertTriangle, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { cn } from "@/libs/shadcn/utils";
import { AdminCount } from "@/pages/admin/components/AdminCount";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { relativeTime } from "@/pages/admin/utils";
import {
  type AirwaySourceKindConfig,
  asContractPolicy,
  asEnvironment
} from "@/services/api/airwayConfig";
import { envLabel, policyLabel } from "../../policyOptions";
import { WIDE_ONLY } from "../../SourceKindTable/layout";

/**
 * A source kind as one line: what is stored for it, how many workspaces diverge, when
 * it last changed, and whether there is an edit waiting to be saved.
 *
 * The line shows the **stored** row, never the draft: it is what airway runs today.
 * The editor under it shows what the operator is changing it to, and "Unsaved" here is
 * the bridge between the two, with its Save, so an edit left in a collapsed row is
 * still one click from saving and never invisible.
 *
 * The kind's name is the row's one toggle button, stretched over the whole line, so
 * a click anywhere opens the row while Save stays a separate button above it.
 */
export function SourceKindLine({
  kind,
  open,
  onToggle,
  editorId,
  dirty,
  saving,
  onSave
}: {
  kind: AirwaySourceKindConfig;
  open: boolean;
  onToggle: () => void;
  editorId: string;
  dirty: boolean;
  saving: boolean;
  onSave: () => void;
}) {
  const id = kind.source_kind;
  const stored = kind.global;
  const policy = asContractPolicy(stored?.contract_policy);
  const env = asEnvironment(stored?.environment);

  return (
    <div className='relative col-span-full grid grid-cols-subgrid items-center px-3 py-2 text-xs transition-colors hover:bg-muted/40'>
      <button
        type='button'
        onClick={onToggle}
        aria-expanded={open}
        aria-controls={open ? editorId : undefined}
        className="flex min-w-0 items-center gap-1.5 text-left font-medium font-mono text-[11px] after:absolute after:inset-0 after:content-[''] focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-ring focus-visible:after:ring-inset"
        data-testid={`admin-airway-toggle-${id}`}
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-90"
          )}
          aria-hidden
        />
        <span className='truncate'>{id}</span>
      </button>

      <StoredValue
        stored={stored?.contract_policy}
        label={policy && policyLabel(policy)}
        fallback='Permissive'
        data-testid={`admin-airway-policy-${id}`}
      />
      <StoredValue
        stored={stored?.environment}
        label={env && envLabel(env)}
        fallback='Production'
        data-testid={`admin-airway-env-${id}`}
      />

      <AdminCount
        value={kind.overrides.length}
        className='text-right'
        data-testid={`admin-airway-override-count-${id}`}
      />

      <span
        className={cn(
          WIDE_ONLY,
          "justify-end whitespace-nowrap text-muted-foreground tabular-nums"
        )}
        title={stored ? new Date(stored.updated_at).toLocaleString() : undefined}
      >
        {stored ? relativeTime(stored.updated_at) : "never"}
      </span>

      <span className='flex items-center justify-end gap-2'>
        {dirty || saving ? (
          <>
            <span
              className='flex items-center gap-1.5 whitespace-nowrap'
              data-testid={`admin-airway-unsaved-${id}`}
            >
              <span className={cn("size-1.5 rounded-full", ADMIN_TONE.warn.dot)} aria-hidden />
              Unsaved
            </span>
            {/* `relative z-10` lifts it above the stretched toggle, so it saves
                rather than collapsing the row. */}
            <Button
              type='button'
              size='sm'
              className='relative z-10 h-6 px-2 text-xs'
              disabled={!dirty || saving}
              onClick={onSave}
              data-testid={`admin-airway-save-${id}`}
            >
              {saving && <Spinner className='size-3' />}
              Save
            </Button>
          </>
        ) : null}
      </span>
    </div>
  );
}

/**
 * One stored field. Unset reads as the default it resolves to, faded, so a column of
 * defaults recedes and an explicit value stands out. A spelling this build cannot read
 * is the one case drawn in colour: airway refuses it, so that kind is failing now.
 */
function StoredValue({
  stored,
  label,
  fallback,
  "data-testid": dataTestId
}: {
  stored: string | null | undefined;
  /** The known value's label, or `null` when `stored` is a spelling this build doesn't know. */
  label: string | null;
  fallback: string;
  "data-testid": string;
}) {
  if (stored == null) {
    return (
      <span className='truncate text-muted-foreground/70' data-testid={dataTestId}>
        {fallback} <span className='text-muted-foreground/50'>· default</span>
      </span>
    );
  }
  if (label === null) {
    return (
      <span
        className={cn("flex min-w-0 items-center gap-1 font-mono", ADMIN_TONE.danger.text)}
        title='A value this build does not recognise. Airway refuses it at run time.'
        data-testid={dataTestId}
      >
        <AlertTriangle className='size-3 shrink-0' aria-hidden />
        <span className='truncate'>{stored}</span>
      </span>
    );
  }
  return (
    <span className='truncate font-medium' data-testid={dataTestId}>
      {label}
    </span>
  );
}
