import { useId } from "react";
import { useAppEnvironments } from "@/hooks/api/customApps/useAppEnvironments";
import { isForbidden } from "@/hooks/api/httpStatus";
import { PRODUCTION } from "@/types/appEnvironments";
import { environmentLabel, STAGING } from "./environmentLabel";

/**
 * Which environment's secrets the pane shows: production (the default),
 * staging, or one of the app's sandboxes.
 *
 * Production and staging always exist, so they are offered before (and
 * whatever happens to) the environments fetch; sandboxes come from it, minus
 * any being torn down. A 403 on that fetch means the viewer may not open the
 * app's non-production environments at all (oxy-authz `AppNonProduction`):
 * production stays, and the picker says why nothing else is offered rather
 * than silently offering less.
 */
export const EnvironmentPicker = ({
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
  // A sandbox deleted while selected stays named until the pane falls back to
  // production (`Secrets`), which it does as soon as this list drops it.
  if (!names.includes(value)) names.push(value);

  return (
    <div className='flex flex-col gap-1'>
      <div className='flex items-center gap-2'>
        <label htmlFor={id} className='text-muted-foreground text-xs'>
          Environment
        </label>
        <select
          id={id}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          className='h-7 border border-input bg-background px-2 text-xs'
          data-testid='admin-app-secrets-environment'
        >
          {names.map((name) => (
            <option key={name} value={name}>
              {environmentLabel(name)}
            </option>
          ))}
        </select>
      </div>
      {locked && (
        <p className='text-muted-foreground text-xs' data-testid='admin-app-secrets-nonprod-locked'>
          Only production is offered: staging and sandboxes need permission to open this app&rsquo;s
          non-production environments.
        </p>
      )}
    </div>
  );
};
