import type { ReactNode } from "react";
import { oxygenFirst } from "@/components/Notifications/utils";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useNotificationSettings, useUpdateNotificationSettings } from "@/hooks/api/notifications";
import { Devices } from "./components/Devices";
import { PauseRow } from "./components/PauseRow";
import { QuietHoursRow } from "./components/QuietHoursRow";
import { SourceGroup } from "./components/SourceGroup";

const NOTE = "text-muted-foreground text-sm";

/**
 * Account → Notifications: everything the signed-in person decides about what
 * reaches them, top down — pause and quiet hours for everything, each device
 * push reaches, then a switch per channel at every level of each source's
 * tree. The inbox always gets everything. Needs no org or workspace: every
 * setting is the caller's own.
 */
export default function AccountNotifications() {
  const settings = useNotificationSettings();
  const update = useUpdateNotificationSettings();
  const doc = settings.data;

  let schedule: ReactNode = null;
  let tree: ReactNode = null;
  if (settings.isPending) {
    schedule = <Skeleton className='h-24 w-full' />;
  } else if (settings.isError || !doc) {
    schedule = <p className={NOTE}>Couldn't load your notification settings</p>;
  } else {
    schedule = (
      <div className='divide-y rounded-lg border'>
        <PauseRow
          global={doc.global}
          onChange={(paused_until) => update.mutate({ global: { paused_until } })}
        />
        <QuietHoursRow global={doc.global} onChange={(global) => update.mutate({ global })} />
      </div>
    );
    tree =
      doc.sources.length === 0 ? (
        <p className={NOTE} data-testid='notifications-prefs-empty'>
          Nothing sends you notifications yet
        </p>
      ) : (
        <div className='flex flex-col gap-3'>
          {!doc.reach.email && (
            <p className='text-muted-foreground text-xs' data-testid='notifications-no-email'>
              No email on this account
            </p>
          )}
          {oxygenFirst(doc.sources).map((source) => (
            <SourceGroup key={source.app?.id ?? "oxygen"} source={source} reach={doc.reach} />
          ))}
        </div>
      );
  }

  return (
    <div className='flex flex-col gap-5' data-testid='settings-account-notifications'>
      {schedule}
      <Devices />
      {tree}
    </div>
  );
}
