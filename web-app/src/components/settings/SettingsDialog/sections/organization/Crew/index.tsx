import { useOrgAppAccessList } from "@/hooks/api/appAccess";
import { useFrontlineDevices, useFrontlineWorkers } from "@/hooks/api/organizations";
import { kioskState } from "@/libs/frontline";
import type { Organization, OrgRole } from "@/types/organization";
import NoAccessNotice from "../../../components/NoAccessNotice";
import { KiosksPane } from "./components/KiosksPane";
import { WorkersPane } from "./components/WorkersPane";

interface CrewSectionProps {
  org: Organization;
  viewerRole: OrgRole;
}

/**
 * The org's frontline crew: workers who sign in with a PIN on a shared tablet,
 * and the tablets themselves.
 *
 * Two stacked panes rather than tabs, the way Members stacks its roster over
 * pending invitations — a kiosk without workers and a worker without a kiosk
 * are each half a setup, and an admin should see both halves at once.
 *
 * The nav gate (`requires: "orgAdmin"`) is the primary check; `canManage` is
 * the second line, and it also holds the queries back so a deep link never
 * fires three admin-only reads on a member's behalf.
 */
export default function CrewSection({ org, viewerRole }: CrewSectionProps) {
  const orgId = org.id;
  const canManage = viewerRole === "owner" || viewerRole === "admin";

  const workers = useFrontlineWorkers(orgId, canManage);
  const devices = useFrontlineDevices(orgId, canManage);
  // The same list App access edits: every org app, including ones the viewer
  // can't personally open — an admin grants apps they don't use.
  const { data: apps } = useOrgAppAccessList(orgId, canManage);

  if (!canManage) {
    return (
      <NoAccessNotice>You need to be an organization owner or admin to manage crew.</NoAccessNotice>
    );
  }

  const workerCount = workers.data?.length;
  // Revoked rows stay in the table as history, but a revoked kiosk is not
  // one a tablet can sign in on, so the summary doesn't count it.
  const kioskCount = devices.data?.filter((device) => kioskState(device) !== "revoked").length;
  const counts =
    workerCount !== undefined && kioskCount !== undefined
      ? `${workerCount} ${workerCount === 1 ? "worker" : "workers"} · ${kioskCount} ${kioskCount === 1 ? "kiosk" : "kiosks"}`
      : null;

  return (
    <div className='flex flex-col gap-5' data-testid='settings-crew'>
      {counts && <p className='text-muted-foreground text-sm'>{counts}</p>}

      <WorkersPane
        orgId={orgId}
        apps={apps ?? []}
        workers={workers.data ?? []}
        isPending={workers.isPending}
        isError={workers.isError}
      />

      <KiosksPane
        orgId={orgId}
        orgSlug={org.slug}
        apps={apps ?? []}
        devices={devices.data ?? []}
        isPending={devices.isPending}
        isError={devices.isError}
      />
    </div>
  );
}
