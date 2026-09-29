import { ExternalLink, LogOut, MonitorSmartphone } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";
import type { BoundKioskDevice } from "@/types/frontline";
import type { Organization } from "@/types/organization";
import { isOrgAdmin, kioskAppName, kioskSummary } from "../utils";
import LeaveKioskConfirm from "./LeaveKioskConfirm";

interface Props {
  device: BoundKioskDevice;
  /** The viewer's membership in the kiosk's org, when they hold one. */
  org: Organization | undefined;
  onLeave: () => void;
  isLeaving: boolean;
  leaveError: unknown;
}

/** An enrolled kiosk: what it is, where it goes, and — for its org's admins — the way out. */
export default function KioskPanel({ device, org, onLeave, isLeaving, leaveError }: Props) {
  const [confirming, setConfirming] = useState(false);
  const canLeave = isOrgAdmin(org);
  const appName = kioskAppName(device.returnTo);

  return (
    <div className='flex flex-col gap-6' data-testid='kiosk-manage-kiosk'>
      <div className='flex flex-col gap-2'>
        <MonitorSmartphone className='h-8 w-8 text-muted-foreground' aria-hidden='true' />
        <h1 className='font-semibold text-xl'>Manage this tablet</h1>
        <p className='font-medium' data-testid='kiosk-manage-summary'>
          {kioskSummary(device)}
        </p>
        <p className='text-muted-foreground text-sm'>
          This browser is a store kiosk: its login page is the crew&apos;s sign-in, in every Oxygen
          app it opens.
        </p>
      </div>

      <div className='flex flex-col gap-2'>
        {device.returnTo && (
          <Button asChild size='lg'>
            <a href={device.returnTo} data-testid='kiosk-manage-open-app'>
              <ExternalLink className='h-4 w-4' />
              {appName ? `Open ${appName}` : "Open the kiosk's app"}
            </a>
          </Button>
        )}
        <Button asChild size='lg' variant={device.returnTo ? "outline" : "default"}>
          <Link to={ROUTES.ROOT} data-testid='kiosk-manage-open-oxygen'>
            Open Oxygen
          </Link>
        </Button>
        {canLeave && !confirming && (
          <Button
            size='lg'
            variant='ghost'
            className='text-destructive hover:text-destructive'
            onClick={() => setConfirming(true)}
            data-testid='kiosk-manage-leave'
          >
            <LogOut className='h-4 w-4' />
            Leave kiosk mode
          </Button>
        )}
      </div>

      {canLeave && confirming && (
        <LeaveKioskConfirm
          device={device}
          onConfirm={onLeave}
          onCancel={() => setConfirming(false)}
          isLeaving={isLeaving}
          error={leaveError}
        />
      )}
      {!canLeave && (
        <p className='text-muted-foreground text-xs' data-testid='kiosk-manage-admin-only'>
          Only an owner or admin of {device.orgName} can take this browser out of kiosk mode.
        </p>
      )}
    </div>
  );
}
