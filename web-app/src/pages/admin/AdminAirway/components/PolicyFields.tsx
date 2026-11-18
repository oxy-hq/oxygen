import type { ReactNode } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { cn } from "@/libs/shadcn/utils";
import type { AirwayContractPolicy, AirwayEnvironment } from "@/services/api/airwayConfig";
import { ENV_OPTIONS, POLICY_OPTIONS } from "../policyOptions";

/** Sentinel select value for "no explicit value stored — inherit the built-in default." */
const INHERIT = "inherit";

interface PolicyFieldsProps {
  /** Distinguishes testids when several `PolicyFields` instances render on one page (per kind, or inside a dialog). */
  idSuffix: string;
  draftPolicy: AirwayContractPolicy | null;
  draftEnv: AirwayEnvironment | null;
  onPolicyChange: (value: AirwayContractPolicy | null) => void;
  onEnvChange: (value: AirwayEnvironment | null) => void;
  /**
   * What "no explicit value" resolves to, in this component's context. A
   * kind's global row has nothing above it but airway's hardcoded default
   * (permissive/production); a workspace override's "inherit" is that
   * kind's global row, which may itself be something else entirely — so
   * the two contexts need different, non-misleading copy here.
   */
  inheritPolicyLabel?: string;
  inheritEnvLabel?: string;
  inheritPolicyHelp?: string;
  className?: string;
}

/**
 * The two editable selects — `contract_policy` and `environment`. Neither
 * onChange saves; the caller treats any change here as "arm Save, drop the
 * preview" (see `SourceKindRow`'s `onPolicyChange` / `onEnvChange`, which close the
 * preview). Shared by the
 * global row (`SourceKindRow`) and the per-workspace override form
 * (`AddOverrideDialog`) — both are "two independently-nullable fields",
 * which is exactly what makes a sparse override (only one field set)
 * expressible for free: `null` is just "inherit" either way.
 */
export function PolicyFields({
  idSuffix,
  draftPolicy,
  draftEnv,
  onPolicyChange,
  onEnvChange,
  inheritPolicyLabel = "Inherit default (permissive)",
  inheritEnvLabel = "Inherit default (production)",
  inheritPolicyHelp,
  className
}: PolicyFieldsProps) {
  return (
    <div className={cn("grid gap-4 sm:grid-cols-2", className)}>
      <Field label='Contract policy'>
        <Select
          value={draftPolicy ?? INHERIT}
          onValueChange={(v) => onPolicyChange(v === INHERIT ? null : (v as AirwayContractPolicy))}
        >
          <SelectTrigger className='w-full' data-testid={`admin-airway-policy-select-${idSuffix}`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={INHERIT}>{inheritPolicyLabel}</SelectItem>
            {POLICY_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {/* The chosen policy's one-line description. Inheriting says nothing more: the
            select already reads "Inherit default (permissive)", and repeating it under
            every kind was the same sentence four times down the page. */}
        {draftPolicy ? (
          <p className='mt-1 text-muted-foreground text-xs'>
            {POLICY_OPTIONS.find((o) => o.value === draftPolicy)?.description}
          </p>
        ) : inheritPolicyHelp ? (
          <p className='mt-1 text-muted-foreground text-xs'>{inheritPolicyHelp}</p>
        ) : null}
      </Field>

      <Field label='Environment'>
        <Select
          value={draftEnv ?? INHERIT}
          onValueChange={(v) => onEnvChange(v === INHERIT ? null : (v as AirwayEnvironment))}
        >
          <SelectTrigger className='w-full' data-testid={`admin-airway-env-select-${idSuffix}`}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={INHERIT}>{inheritEnvLabel}</SelectItem>
            {ENV_OPTIONS.map((opt) => (
              <SelectItem key={opt.value} value={opt.value}>
                {opt.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
    </div>
  );
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <span className='mb-1.5 block font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
        {label}
      </span>
      {children}
    </div>
  );
}
