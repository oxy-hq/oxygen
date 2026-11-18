import {
  type AirwayConfigValues,
  type AirwaySourceKindConfig,
  asContractPolicy,
  asEnvironment
} from "@/services/api/airwayConfig";

/**
 * The stored fields on a kind's global row that this build cannot read, as
 * `Contract policy = "x"` phrases. Empty when the row is absent or every field
 * is a known spelling (or unset).
 *
 * The column is free text (see `asContractPolicy`), so a row written by raw SQL
 * or by a build that knew another spelling arrives outside the union. Airway
 * refuses such a value at run time rather than falling back, so a non-empty
 * result means that kind's pipelines are already failing — which is why the
 * row says so in its own cells and the page counts it.
 */
export function unrecognisedFields(values: AirwayConfigValues | null | undefined): string[] {
  if (!values) return [];
  const out: string[] = [];
  if (values.contract_policy != null && asContractPolicy(values.contract_policy) === null) {
    out.push(`Contract policy = "${values.contract_policy}"`);
  }
  if (values.environment != null && asEnvironment(values.environment) === null) {
    out.push(`Environment = "${values.environment}"`);
  }
  return out;
}

/** The numbers `/admin/airway` leads with. Every count is over the kinds' **stored** rows. */
export interface AirwaySummary {
  kinds: number;
  /** Kinds with a global row at all. The rest run airway's built-in default. */
  configured: number;
  /** Kinds whose global row stores a policy stricter than `permissive`. */
  tightened: number;
  /** Kinds whose global row stores `sandbox`, which airway refuses some connectors under. */
  sandbox: number;
  /** Override rows across every kind. */
  overrides: number;
  /** Distinct workspaces those rows belong to: one workspace can override several kinds. */
  overrideWorkspaces: number;
  /** Kinds whose global row holds a spelling this build cannot read (see `unrecognisedFields`). */
  unrecognised: number;
}

export function summarizeKinds(kinds: AirwaySourceKindConfig[]): AirwaySummary {
  const workspaces = new Set<string>();
  let overrides = 0;
  for (const kind of kinds) {
    for (const o of kind.overrides) {
      overrides += 1;
      workspaces.add(o.workspace_id);
    }
  }
  const policy = (k: AirwaySourceKindConfig) => asContractPolicy(k.global?.contract_policy);
  return {
    kinds: kinds.length,
    configured: kinds.filter((k) => k.global !== null).length,
    tightened: kinds.filter((k) => {
      const p = policy(k);
      return p !== null && p !== "permissive";
    }).length,
    sandbox: kinds.filter((k) => asEnvironment(k.global?.environment) === "sandbox").length,
    overrides,
    overrideWorkspaces: workspaces.size,
    unrecognised: kinds.filter((k) => unrecognisedFields(k.global).length > 0).length
  };
}
