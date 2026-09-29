import type { ReactNode } from "react";
import OxyLogo from "@/components/OxyLogo";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useLeaveKioskMode, useOrgs } from "@/hooks/api/organizations";
import { useKioskDevice } from "@/hooks/auth/useFrontline";
import KioskPanel from "./components/KioskPanel";
import LeftKiosk from "./components/LeftKiosk";
import NotAKiosk from "./components/NotAKiosk";

/**
 * `/kiosk` — "Manage this tablet". Where "Sign in as an admin" on a kiosk's
 * login page lands, and the one place a browser can be taken out of kiosk mode
 * from the browser itself.
 *
 * Three states: not a kiosk (and where one is revoked instead); a kiosk, with
 * its app, Oxygen, and — for an owner or admin of the kiosk's org — "Leave
 * kiosk mode"; and left. The admin check is a display hint read from `GET
 * /orgs`; the server's `OrgAdmin` guard decides.
 *
 * The left state is held by the mutation, not the probe: a successful leave
 * refetches the probe, which then answers "not a kiosk", and the page must keep
 * saying what just happened rather than flip to the other state.
 */
export default function KioskManagePage() {
  const { data: device, isPending } = useKioskDevice();
  const { data: orgs } = useOrgs();
  const leave = useLeaveKioskMode();

  if (leave.isSuccess) {
    return (
      <Shell>
        <LeftKiosk />
      </Shell>
    );
  }
  if (isPending) {
    return (
      <Shell>
        <div className='flex justify-center py-10' data-testid='kiosk-manage-probing'>
          <Spinner />
        </div>
      </Shell>
    );
  }
  if (!device?.bound) {
    return (
      <Shell>
        <NotAKiosk orgs={orgs} />
      </Shell>
    );
  }
  const org = orgs?.find((o) => o.slug === device.org);
  return (
    <Shell>
      <KioskPanel
        device={device}
        org={org}
        onLeave={() => org && leave.mutate({ orgId: org.id })}
        isLeaving={leave.isPending}
        leaveError={leave.error}
      />
    </Shell>
  );
}

function Shell({ children }: { children: ReactNode }) {
  return (
    <div className='flex min-h-screen w-full flex-col bg-background' data-testid='kiosk-manage'>
      <div className='flex items-center gap-2 p-6 font-medium'>
        <OxyLogo />
        <span className='truncate text-sm'>Oxygen</span>
      </div>
      <div className='flex flex-1 items-start justify-center px-6 pt-10 pb-16 md:pt-20'>
        <div className='w-full max-w-md'>{children}</div>
      </div>
    </div>
  );
}
