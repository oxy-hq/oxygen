import { RotateCcw } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useResetEnvironmentPlanes } from "@/hooks/api/customApps/useAppEnvironments";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppEnvironment } from "@/services/api/customApps";
import { LifecycleConfirm } from "../../../../../../AppLifecycle";
import {
  chosenPlanes,
  fetchHosts,
  PLANE_LABEL,
  PLANE_ORDER,
  resetCopy,
  resettablePlanes
} from "../../../utils";

/**
 * Every plane and the mode it resolves to. A plane the environment chose itself
 * is marked `set`; the rest are its kind's defaults. A reset sits beside each
 * plane that has one.
 */
export const PlanesTable = ({ appId, env }: { appId: string; env: AppEnvironment }) => {
  const reset = useResetEnvironmentPlanes(appId);
  const [confirming, setConfirming] = useState<string | null>(null);
  const chosen = new Set(chosenPlanes(env).map((c) => c.plane));
  const resettable = new Set(resettablePlanes(env));
  const hosts = fetchHosts(env);

  if (env.planes_error) {
    return (
      <p
        className={cn("whitespace-pre-wrap break-words font-mono text-xs", ADMIN_TONE.danger.text)}
        data-testid='admin-app-env-planes-unreadable'
      >
        Its planes could not be read: {env.planes_error}
      </p>
    );
  }

  return (
    <>
      <ul
        className='flex flex-col divide-y divide-border/50 text-xs'
        data-testid='admin-app-env-planes'
      >
        {PLANE_ORDER.map((plane) => (
          <li
            key={plane}
            className='flex items-center gap-2 py-1.5'
            data-testid={`admin-app-env-plane-row-${plane}`}
          >
            <span className='w-40 shrink-0 text-muted-foreground'>{PLANE_LABEL[plane]}</span>
            <span className='min-w-0 flex-1 truncate font-mono'>
              {env.planes?.[plane] ?? "—"}
              {plane === "fetch" && hosts.length > 0 ? ` → ${hosts.join(", ")}` : ""}
            </span>
            <span className={cn("w-6 shrink-0", ADMIN_TONE.info.text)}>
              {chosen.has(plane) ? "set" : ""}
            </span>
            <span className='flex w-16 shrink-0 justify-end'>
              {resettable.has(plane) && (
                <Button
                  variant='ghost'
                  size='sm'
                  className='h-6 gap-1 px-1.5 text-muted-foreground text-xs'
                  disabled={reset.isPending}
                  onClick={() => setConfirming(plane)}
                  data-testid={`admin-app-env-reset-${plane}`}
                >
                  <RotateCcw className='size-3' />
                  Reset
                </Button>
              )}
            </span>
          </li>
        ))}
      </ul>
      <LifecycleConfirm
        testId='admin-app-env-reset-confirm'
        copy={
          confirming
            ? {
                title: `Reset ${PLANE_LABEL[confirming]} in ${env.name}?`,
                description: resetCopy(confirming, env),
                confirm: "Reset"
              }
            : null
        }
        pending={reset.isPending}
        onConfirm={() =>
          confirming &&
          reset.mutate(
            { name: env.name, planes: [confirming] },
            { onSuccess: () => setConfirming(null) }
          )
        }
        onCancel={() => setConfirming(null)}
      />
    </>
  );
};
