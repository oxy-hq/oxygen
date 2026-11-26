import { useId } from "react";
import { useAppEnvironments } from "@/hooks/api/customApps/useAppEnvironments";
import { useSecretsGaps } from "@/hooks/api/customApps/useAppSecrets";
import { isForbidden } from "@/hooks/api/httpStatus";
import { cn } from "@/libs/shadcn/utils";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { PRODUCTION } from "@/types/appEnvironments";
import { STAGING } from "../AppDetail/components/Secrets/environmentLabel";
import { environmentOption, environmentScope } from "./environmentScope";

/**
 * Which environment this console is acting on, across the top of the page, with
 * the switch that changes it.
 *
 * Functions (their builds, invocations and Run now) and Secrets follow it. Issues
 * and the availability panel are production's only, and the strip says so, so a
 * panel that did not move with the switch is not mistaken for one that did.
 *
 * Each option carries that environment's missing required secrets, the "not set
 * in staging" question asked where the environment is chosen rather than one
 * environment at a time inside the Secrets panel.
 *
 * A viewer who may not open non-production (a 403 on the environments list) sees
 * production named and no switch: there is nothing else they could pick.
 */
export const EnvironmentStrip = ({
  appId,
  value,
  onChange
}: {
  appId: string;
  value: string;
  onChange: (environment: string) => void;
}) => {
  const id = useId();
  const environments = useAppEnvironments(appId);
  const locked = isForbidden(environments.error);
  const sandboxes = (environments.data ?? [])
    .filter((e) => e.kind === "dev" && e.status !== "deleting")
    .map((e) => e.name);
  const names = locked ? [PRODUCTION] : [PRODUCTION, STAGING, ...sandboxes];
  if (!names.includes(value)) names.push(value);
  const gaps = useSecretsGaps(appId, names);
  const scope = environmentScope(value);
  const tone = ADMIN_TONE[scope.tone];

  return (
    <div
      className={cn(
        "mb-3 flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-1.5 text-xs ring-1 ring-inset",
        tone.bg,
        tone.ring
      )}
      data-testid='apps-console-environment'
      data-environment={value}
    >
      <span className={cn("font-semibold", tone.text)} data-testid='apps-console-environment-name'>
        {scope.label}
      </span>
      <span className='flex min-w-0 flex-1 items-center gap-1.5 text-muted-foreground'>
        <span className='truncate'>{scope.what}</span>
        <AdminInfo>
          Pages that are one environment's follow this switch: Functions, Events, Data, Secrets.
          Issues and availability always show production.
        </AdminInfo>
      </span>
      {!locked && (
        <span className='flex items-center gap-1.5'>
          <label htmlFor={id} className='text-muted-foreground'>
            Environment
          </label>
          <select
            id={id}
            value={value}
            onChange={(e) => onChange(e.target.value)}
            className='h-7 border border-input bg-background px-2 text-xs'
            data-testid='apps-console-environment-select'
          >
            {names.map((name) => (
              <option key={name} value={name}>
                {environmentOption(name, gaps[name])}
              </option>
            ))}
          </select>
        </span>
      )}
    </div>
  );
};
