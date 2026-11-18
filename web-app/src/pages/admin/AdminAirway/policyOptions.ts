import type { AirwayContractPolicy, AirwayEnvironment } from "@/services/api/airwayConfig";

/**
 * The two admission axes' options, in the words the page uses for them. One list for the
 * selects (`PolicyFields`) and the table row that reads the stored value back
 * (`SourceKindLine`), so a row and the select under it never name a policy two ways.
 */
export const POLICY_OPTIONS: Array<{
  value: AirwayContractPolicy;
  label: string;
  description: string;
}> = [
  {
    value: "permissive",
    label: "Permissive",
    description: "Every resource is admitted, declared or not — today's default."
  },
  {
    value: "forbid_opaque",
    label: "Forbid opaque",
    description:
      "Rejects resources whose vendor contract is a confirmed opaque fact. Undeclared resources still pass."
  },
  {
    value: "require_declared",
    label: "Require declared",
    description:
      "Rejects anything without a checked immutable or versioned contract — opaque and undeclared alike."
  }
];

export const ENV_OPTIONS: Array<{ value: AirwayEnvironment; label: string }> = [
  { value: "production", label: "Production" },
  { value: "sandbox", label: "Sandbox" }
];

export function policyLabel(value: AirwayContractPolicy): string {
  return POLICY_OPTIONS.find((o) => o.value === value)?.label ?? value;
}

export function envLabel(value: AirwayEnvironment): string {
  return ENV_OPTIONS.find((o) => o.value === value)?.label ?? value;
}
