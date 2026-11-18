import { type AdminKpi, AdminKpiStrip } from "@/pages/admin/components/AdminKpiStrip";
import type { AirwaySourceKindConfig } from "@/services/api/airwayConfig";
import { summarizeKinds } from "../summary";

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;

/**
 * What the policy tier looks like at a glance: how much of it is set at all, how much
 * of it is strict, and how much diverges per workspace. Every cell fades at zero, so a
 * deployment running pure defaults reads as four quiet zeros. "Unrecognised" appears
 * only when something is: it is a finding, and a finding that is not there is not a cell.
 */
export function AirwayKpis({ kinds }: { kinds: AirwaySourceKindConfig[] }) {
  const s = summarizeKinds(kinds);
  const items: AdminKpi[] = [
    {
      id: "configured",
      label: "Configured",
      value: s.configured,
      caption: `of ${plural(s.kinds, "source kind")}`,
      quiet: s.configured === 0,
      info: "Kinds with a stored policy row. The rest run airway's built-in default: permissive, production."
    },
    {
      id: "tightened",
      label: "Tightened",
      value: s.tightened,
      caption: "stricter than permissive",
      quiet: s.tightened === 0,
      info: "Kinds whose stored policy rejects some resources: Forbid opaque or Require declared."
    },
    {
      id: "sandbox",
      label: "Sandbox",
      value: s.sandbox,
      caption: "admitted as sandbox",
      quiet: s.sandbox === 0,
      info: "Kinds stored with the Sandbox environment. Airway refuses some connectors under Sandbox."
    },
    {
      id: "overrides",
      label: "Overrides",
      value: s.overrides,
      caption: s.overrides > 0 ? `in ${plural(s.overrideWorkspaces, "workspace")}` : undefined,
      quiet: s.overrides === 0,
      info: "Workspace rows that replace a kind's policy for one workspace."
    }
  ];
  if (s.unrecognised > 0) {
    items.push({
      id: "unrecognised",
      label: "Unrecognised",
      value: s.unrecognised,
      tone: "danger",
      caption: "kinds failing every run",
      info: "A stored value this build does not know. Airway refuses it at run time instead of falling back, so the kind's pipelines fail until it is replaced."
    });
  }
  return <AdminKpiStrip items={items} data-testid='admin-airway-kpis' />;
}
