import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { cn } from "@/libs/shadcn/utils";
import type { AirwayDeploymentValues } from "@/services/api/airwayConfig";
import { type DeploymentField, UNSET } from "./fields";

/**
 * How an unset setting reads, everywhere it appears. Never "0", never blank —
 * absence means airway's compiled-in value, which is a definite state and has
 * to look like one.
 */
const DEFAULT_LABEL = "airway default";

/** The installed column: a value, or the default marker. */
function InstalledCell({
  field,
  installed,
  observed
}: {
  field: DeploymentField;
  installed: AirwayDeploymentValues | null;
  observed: boolean;
}) {
  if (!observed) {
    return <span className='text-muted-foreground/70 text-xs italic'>not observed</span>;
  }
  const value = installed?.[field.key] ?? null;
  if (value === null) {
    return <span className='text-muted-foreground text-xs'>{DEFAULT_LABEL}</span>;
  }
  // `tabular-nums` on a boolean is inert, so one span serves both — but the
  // long path strings need to wrap rather than blow the column out.
  return <span className='break-all text-foreground text-xs tabular-nums'>{String(value)}</span>;
}

/**
 * One setting: its editable value, what the answering process installed, and
 * whether the two differ.
 *
 * The drift mark is per row rather than only in the banner so an operator
 * scanning ten fields can see which one owes a restart without reading prose.
 * Warning tokens, not emerald — emerald is reserved for workflow-node success.
 */
export function DeploymentFieldRow({
  field,
  draft,
  installed,
  observed,
  drifted,
  invalid,
  disabled,
  onChange
}: {
  field: DeploymentField;
  draft: string;
  installed: AirwayDeploymentValues | null;
  observed: boolean;
  drifted: boolean;
  invalid: boolean;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  const testId = `admin-airway-deployment-field-${field.key}`;
  return (
    <div
      className='grid grid-cols-1 gap-2 border-border/50 border-b py-2.5 last:border-b-0 sm:grid-cols-[minmax(0,1fr)_10rem_9rem]'
      data-testid={testId}
    >
      <div className='min-w-0'>
        <div className='flex items-center gap-1.5'>
          <span className='font-medium text-foreground text-xs'>{field.label}</span>
          {field.unit && (
            <span className='text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
              {field.unit}
            </span>
          )}
          {drifted && (
            <span
              className='inline-flex items-center gap-1 border border-warning/40 bg-warning/10 px-1 py-px font-medium text-[10px] text-warning'
              data-testid={`${testId}-drift`}
            >
              <span className='size-1.5 rounded-full bg-warning' />
              differs
            </span>
          )}
        </div>
        <p className='mt-0.5 text-muted-foreground text-xs'>{field.help}</p>
        {invalid && (
          <p className='mt-0.5 text-destructive text-xs' data-testid={`${testId}-invalid`}>
            Not a {field.kind === "decimal" ? "number" : "whole number"} — clear the field to take
            airway's default.
          </p>
        )}
      </div>

      {field.kind === "boolean" ? (
        <Select
          value={draft === UNSET ? "default" : draft}
          disabled={disabled}
          onValueChange={(v) => onChange(v === "default" ? UNSET : v)}
        >
          <SelectTrigger className='h-8 w-full text-xs' data-testid={`${testId}-input`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            <SelectItem value='default'>{DEFAULT_LABEL}</SelectItem>
            <SelectItem value='true'>true</SelectItem>
            <SelectItem value='false'>false</SelectItem>
          </SelectContent>
        </Select>
      ) : (
        <Input
          className={cn("h-8 text-xs", invalid && "border-destructive")}
          value={draft}
          disabled={disabled}
          inputMode={field.kind === "text" ? "text" : "decimal"}
          // The placeholder carries the "empty means default" rule at the one
          // moment an operator is about to clear the field.
          placeholder={DEFAULT_LABEL}
          onChange={(e) => onChange(e.target.value)}
          data-testid={`${testId}-input`}
        />
      )}

      <div className='flex items-center' data-testid={`${testId}-installed`}>
        <InstalledCell field={field} installed={installed} observed={observed} />
      </div>
    </div>
  );
}
