import { ShieldAlert } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  useAdminOltpStatus,
  useDeprovisionOltp,
  useOltpCredentials,
  useProvisionOltp,
  useSetOltpVisibility,
  useTestOltpConnection
} from "@/hooks/api/oltp/useAdminOltp";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { OltpConnectionTest } from "./components/OltpConnectionTest";
import { OltpConnectPanel } from "./components/OltpConnectPanel";
import { OltpDangerZone } from "./components/OltpDangerZone";
import { OltpIdentityBar } from "./components/OltpIdentityBar";
import { OltpSchemasTable } from "./components/OltpSchemasTable";
import { OltpUnprovisioned } from "./components/OltpUnprovisioned";

/**
 * Everything an operator does to one org's OLTP database, in one panel.
 *
 * Shared, because there are two ways in and they must not drift: the org
 * detail page (OLTP as one section among many) and the OLTP fleet page (a
 * tenant selected from the list). It used to live under `AdminOrgDetail`, so
 * the fleet page could only link at it.
 *
 * **One identity line, then two columns.** This was a stack of three bordered
 * cards under a field grid — 578px, with the danger zone below the fold on a
 * laptop. Schemas and Connect are independent and each fits half the width, so
 * side by side they cost one band instead of two, and nothing needs scrolling
 * to reach.
 *
 * **Branches on `status`, not on `is_provisioned`.** The API sets
 * `is_provisioned` only for `active`, while still returning the real status,
 * host, database and schemas for a `failed` or `pending_delete` tenant —
 * `entity/tenants.rs` says of `Failed` that "the row is kept so operators can
 * see it". Gating the whole panel on `is_provisioned` rendered "No OLTP
 * database" with a Provision button over a tenant that has one, and hid the
 * danger zone in precisely the two states it exists for.
 *
 * **One instance per org.** Both callers re-render the panel in place when the
 * operator picks another org, and a mutation's result belongs to the instance,
 * not to `orgId`: org A's Test connection verdict — or A's minted credentials —
 * stayed on screen under org B's heading. Keying by org gives each its own.
 */
export const OltpTenantPanel = ({ orgId }: { orgId: string }) => (
  <TenantPanel key={orgId} orgId={orgId} />
);

const TenantPanel = ({ orgId }: { orgId: string }) => {
  const { data, isPending, isError, error } = useAdminOltpStatus(orgId);
  const provision = useProvisionOltp(orgId);
  const credentials = useOltpCredentials(orgId);
  const visibility = useSetOltpVisibility(orgId);
  const deprovision = useDeprovisionOltp(orgId);
  const probe = useTestOltpConnection(orgId);

  if (isPending) return <Skeleton className='h-24 w-full' />;
  if (isError) {
    return (
      <AdminEmptyState
        icon={ShieldAlert}
        title='Could not read OLTP status'
        description={error instanceof Error ? error.message : undefined}
      />
    );
  }

  // An empty status is the API's "no row at all"; every other value means a
  // tenant exists and has facts worth showing.
  if (!data.status) return <OltpUnprovisioned provision={provision} />;

  return (
    <div className='flex flex-col gap-3' data-testid='admin-org-oltp'>
      <OltpIdentityBar data={data} />

      {/* Right under the identity line: the status there is Oxy's record, and
          this is the database's own answer to the same question. */}
      <OltpConnectionTest probe={probe} />

      {!data.is_provisioned && (
        <p className='text-xs' data-testid='admin-org-oltp-not-active'>
          This database is <strong>{data.status}</strong>, not active. Re-running Provision
          reconciles it; Deprovision below clears it away.
        </p>
      )}

      {!data.analyst_ready && (
        <p className='text-destructive text-xs'>
          Without the analyst credential, <code className='font-mono'>type: postgres_managed</code>{" "}
          cannot resolve — the SQL IDE and agents will fail to connect. Re-run Provision to mint it.
        </p>
      )}

      <div className='grid grid-cols-1 gap-x-8 gap-y-4 lg:grid-cols-2'>
        <OltpSchemasTable data={data} visibility={visibility} provision={provision} />
        <OltpConnectPanel data={data} credentials={credentials} />
      </div>

      <OltpDangerZone data={data} deprovision={deprovision} />
    </div>
  );
};
