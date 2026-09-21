import { RotateCcw } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent, CardHeader } from "@/components/ui/shadcn/card";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  useAirwayDeploymentConfig,
  useClearAirwayDeploymentConfig,
  useUpsertAirwayDeploymentConfig
} from "@/hooks/api/airwayConfig/useAirwayDeploymentConfig";
import type { AirwayDeploymentConfigResponse } from "@/services/api/airwayConfig";
import { AdminAsync } from "../../components/AdminAsync";
import { formatUpdatedAt } from "../utils";
import { DeploymentFieldRow } from "./DeploymentFieldRow";
import { DeploymentStateBanner } from "./DeploymentStateBanner";
import {
  DEPLOYMENT_FIELDS,
  type DeploymentDraft,
  draftFromValues,
  GROUP_LABELS,
  isDirty,
  UNSET,
  valuesFromDraft
} from "./fields";

const GROUPS = ["transport", "retry", "extraction", "tls"] as const;

/**
 * The `/admin/airway` **Deployment** region — airway's operational tier
 * (`airway_deployment_config`), a singleton row of eight settings.
 *
 * Distinct from the source-kind cards above it in both scope and lifetime:
 * those are per source kind and resolved on every run, this is per *process*
 * and installed once at worker startup. That difference is the whole design
 * here — see `DeploymentStateBanner` for why a save is not live, and why the
 * installed column is labelled with the process that answered rather than
 * presented as the deployment's state.
 */
export function DeploymentConfig() {
  const config = useAirwayDeploymentConfig();

  return (
    <AdminAsync
      query={config}
      noun='the airway deployment config'
      // One tall card, not a list of rows.
      skeleton={<Skeleton className='h-64 w-full' />}
    >
      {/* Keyed on nothing: `DeploymentForm` seeds its draft from `data` once,
          so it must not be reseeded by a background refetch mid-edit — the
          same reason the form is a child component rather than this one. */}
      {(data) => <DeploymentForm data={data} />}
    </AdminAsync>
  );
}

function DeploymentForm({ data }: { data: AirwayDeploymentConfigResponse }) {
  const [draft, setDraft] = useState<DeploymentDraft>(() => draftFromValues(data.configured));
  const upsert = useUpsertAirwayDeploymentConfig();
  const clear = useClearAirwayDeploymentConfig();

  const { values, invalid } = valuesFromDraft(draft);
  const dirty = isDirty(draft, data.configured);
  const pending = upsert.isPending || clear.isPending;
  // `drift.fields` carries airway's key names, which are exactly the field
  // keys — so the per-row mark and the banner can never disagree.
  const drifted = new Set(data.drift.fields);
  const observed = data.installed !== null;

  return (
    <Card data-testid='admin-airway-deployment'>
      <CardHeader className='flex flex-row items-start justify-between gap-4 space-y-0'>
        <div className='min-w-0'>
          <h3 className='font-semibold text-sm'>Deployment</h3>
          <p className='mt-0.5 text-muted-foreground text-xs'>
            {data.configured_row_exists
              ? formatUpdatedAt(data.updated_at)
              : "Never configured — every setting is airway's built-in default"}
          </p>
        </div>
        <div className='flex shrink-0 items-center gap-1.5'>
          {data.configured_row_exists && (
            <Button
              type='button'
              variant='outline'
              size='sm'
              disabled={pending}
              onClick={() => clear.mutate(undefined, { onSuccess: () => setDraft(emptyDraft()) })}
              data-testid='admin-airway-deployment-clear'
              tooltip="Remove the row — every setting returns to airway's built-in default at the next worker restart"
            >
              {clear.isPending ? (
                <Spinner className='size-3.5' />
              ) : (
                <RotateCcw className='size-3.5' />
              )}
              Clear
            </Button>
          )}
          <Button
            type='button'
            size='sm'
            disabled={!dirty || invalid.length > 0 || pending}
            onClick={() => upsert.mutate(values)}
            data-testid='admin-airway-deployment-save'
            tooltip='Stores the values. They take effect when the airway worker process next starts.'
          >
            {upsert.isPending && <Spinner className='size-3.5' />}
            Save
          </Button>
        </div>
      </CardHeader>

      <CardContent className='space-y-4'>
        <DeploymentStateBanner drift={data.drift} scope={data.installed_scope} />

        <div className='hidden text-[10px] text-muted-foreground uppercase tracking-[0.16em] sm:grid sm:grid-cols-[minmax(0,1fr)_10rem_9rem] sm:gap-2'>
          <span>Setting</span>
          <span>Configured</span>
          <span>Installed</span>
        </div>

        {GROUPS.map((group) => (
          <section key={group} data-testid={`admin-airway-deployment-group-${group}`}>
            <h4 className='mb-1 font-medium text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
              {GROUP_LABELS[group]}
            </h4>
            <div>
              {DEPLOYMENT_FIELDS.filter((f) => f.group === group).map((field) => (
                <DeploymentFieldRow
                  key={field.key}
                  field={field}
                  draft={draft[field.key]}
                  installed={data.installed}
                  observed={observed}
                  drifted={drifted.has(field.key)}
                  invalid={invalid.includes(field.key)}
                  disabled={pending}
                  onChange={(value) => setDraft((d) => ({ ...d, [field.key]: value }))}
                />
              ))}
            </div>
          </section>
        ))}
      </CardContent>
    </Card>
  );
}

/** Every field back to [`UNSET`] — what "Clear" leaves behind on screen. */
function emptyDraft(): DeploymentDraft {
  const draft = {} as DeploymentDraft;
  for (const field of DEPLOYMENT_FIELDS) draft[field.key] = UNSET;
  return draft;
}
