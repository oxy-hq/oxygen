import { Layers } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAirwayConfig } from "@/hooks/api/airwayConfig/useAirwayConfig";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { AirwayKpis } from "./components/AirwayKpis";
import { DeploymentConfig } from "./DeploymentConfig";
import { SourceKindTable } from "./SourceKindTable";

/**
 * `/admin/airway` — staff console for airway's configuration. Three regions,
 * and the first two belong to a different tier than the third:
 *
 * 1. **source kinds** — the admission *policy* tier (`contract_policy`,
 *    `environment`), per kind, resolved on every run: one line per kind in
 *    `SourceKindTable`, each expanding in place to edit it;
 * 2. **workspace overrides**, inside each kind's expanded row, sparse over that
 *    policy;
 * 3. **Deployment** — airway's *operational* tier, deployment-wide and
 *    installed once per worker process. Different scope, different lifetime,
 *    and crucially not live on save — see `DeploymentConfig`.
 *
 * `max_rewind`, `cursor_lag_floor`, `allow_unversioned_writes` and
 * `partition_repull_budget` appear in none of them: they have zero occurrences
 * in airway's source, so a control for one would be accepted, saved and inert
 * — the exact failure this surface exists to avoid.
 *
 * Tightening a kind's `contract_policy` can silently halt every pipeline
 * whose resources don't satisfy it, so the preview is the guardrail — see
 * `SourceKindRow` for how selects never save implicitly and `PolicyPreview`
 * for how a preview is invalidated the moment a select changes.
 */
export default function AdminAirway() {
  // The whole query, not its `data`: loading, a failed read and "airway reports
  // no source kinds" are three different answers for an operator about to
  // tighten a policy, and only `AdminAsync` keeps them apart.
  const config = useAirwayConfig();

  return (
    <AdminPage
      width='default'
      description="Each source kind's contract policy. Preview before tightening one."
      info="The contract policy each source kind admits pipelines under, plus the deployment-wide operational settings airway installs at worker startup. Tightening a kind's policy can halt every pipeline whose resources don't satisfy it, so preview it first. Click a kind to edit it and its workspace overrides."
      data-testid='admin-airway'
    >
      <AdminAsync
        query={config}
        noun='the airway admission config'
        // The strip, then one bar for the table: its rows are one line each, so
        // the bar is about as tall as the table that replaces it.
        skeleton={
          <div className='space-y-5'>
            <Skeleton className='h-[74px] w-full' />
            <Skeleton className='h-56 w-full' />
          </div>
        }
        isEmpty={(data) => data.kinds.length === 0}
        empty={
          <AdminEmptyState
            icon={Layers}
            title='No known source kinds.'
            description='Airway reports no source kinds on this build, so there is no admission policy to configure.'
          />
        }
      >
        {(data) => (
          <div className='space-y-5'>
            <AirwayKpis kinds={data.kinds} />
            <SourceKindTable kinds={data.kinds} />
          </div>
        )}
      </AdminAsync>

      {/* Its own query, so a failure in the policy tier does not take the
          operational tier down with it (and vice versa) — two tiers, two
          tables, two independent reads. That is also why it sits outside the
          `AdminAsync` above rather than inside its render callback. */}
      <DeploymentConfig />
    </AdminPage>
  );
}
