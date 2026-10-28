import { isAxiosError } from "axios";
import type React from "react";
import { toast } from "sonner";
import { AirhouseLogo } from "@/components/icons";
import useAddAirhouseToConfig from "@/hooks/api/airhouse/useAddAirhouseToConfig";
import useAirhouseConnection from "@/hooks/api/airhouse/useAirhouseConnection";
import useProvisionAirhouse from "@/hooks/api/airhouse/useProvisionAirhouse";
import useAuthConfig from "@/hooks/auth/useAuthConfig";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import SectionHeader from "../../../components/SectionHeader";
import { AirhouseVersionBadge } from "./components/AirhouseVersionBadge";
import { CatalogIndexes } from "./components/CatalogIndexes";
import { ConnectionDetails } from "./components/ConnectionDetails";
import { ProvisionPrompt } from "./components/ProvisionPrompt";

function statusFromError(err: unknown): number | undefined {
  return isAxiosError(err) ? err.response?.status : undefined;
}

const Airhouse: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const orgRole = useCurrentOrg((s) => s.role);
  const { data: authConfig } = useAuthConfig();
  const workspaceId = workspace?.id;
  const { data: connection, isLoading, error } = useAirhouseConnection(workspaceId);
  const provision = useProvisionAirhouse(workspaceId);
  const addToConfig = useAddAirhouseToConfig();

  // Provisioning creates a tenant-wide resource and a service account; only
  // org Owner/Admin should be able to do it. Non-admins still see the page
  // and the read-only connection details once provisioning is done — they
  // just can't trigger the initial setup. In local mode there is no org
  // picker so `useCurrentOrg.role` is never populated; the single seeded
  // guest is always Owner server-side (and the provision endpoint enforces
  // that regardless), so treat local mode as always able to provision.
  const isLocal = authConfig?.mode === "local";
  // With the apps-first experience on, setting Airhouse up is the Oxy team's work
  // (the Factory's): an owner or admin who may not use the Factory reads the
  // connection details and is told who sets it up.
  const appsFirst = useAppsFirstExperience();
  const canProvision =
    isLocal ||
    ((orgRole === "owner" || orgRole === "admin") && (!appsFirst || canUseFactory(workspace)));

  const handleProvision = async (tenantName: string) => {
    try {
      await provision.mutateAsync({ tenantName });
    } catch {
      // Error surfaced inline via `provision.error` in <ProvisionPrompt/>.
    }
  };

  const handleAddToConfig = async (name: string) => {
    try {
      const result = await addToConfig.mutateAsync({ name });
      if (result === "already_present") {
        toast.info("airhouse_managed is already in config.yml");
      } else {
        toast.success(`Added '${name}' database to config.yml — commit to persist the change`);
      }
    } catch (err) {
      toast.error(
        err instanceof Error ? err.message : "Failed to add airhouse_managed to config.yml"
      );
    }
  };

  const renderContent = () => {
    if (isLoading) {
      return <p className='text-muted-foreground text-sm'>Loading…</p>;
    }
    const status = statusFromError(error);
    if (status === 503) {
      return (
        <p className='text-muted-foreground text-sm'>
          Airhouse is not configured for this deployment. Ask an administrator to set the Airhouse
          environment variables and restart the server.
        </p>
      );
    }
    if (!connection?.is_provisioned) {
      if (!canProvision) {
        return (
          <p className='text-muted-foreground text-sm'>
            {appsFirst
              ? "Airhouse isn't set up yet. Your Oxy team sets it up."
              : "Airhouse hasn't been provisioned for this workspace yet. Ask an Owner or Admin to complete setup."}
          </p>
        );
      }
      return (
        <ProvisionPrompt
          onProvision={handleProvision}
          isPending={provision.isPending}
          error={provision.error}
        />
      );
    }
    if (error) {
      return (
        <p className='text-muted-foreground text-sm'>
          Failed to load Airhouse connection details. Try refreshing the page.
        </p>
      );
    }
    return (
      <>
        <ConnectionDetails
          connection={connection}
          onAddToConfig={handleAddToConfig}
          isAddingToConfig={addToConfig.isPending}
        />
        {/* Both catalog-index routes are org-admin-gated server-side
            (`require_workspace_admin`), the GET included — so a non-admin can't
            read this state at all. Rendering it anyway fired a 403 on every
            open and tripped the global denial toast. */}
        {workspaceId && canProvision && <CatalogIndexes workspaceId={workspaceId} />}
      </>
    );
  };

  // Deliberately open to any org member: the credential this panel fetches is
  // minted per-user and role-scoped, so a member receives their own read-only
  // `Reader` credential rather than a shared secret. The admin-only pieces
  // (provisioning, catalog indexes) are gated individually above.
  return (
    <div className='flex flex-col gap-5'>
      <SectionHeader
        title={
          <span className='flex items-center gap-2'>
            <AirhouseLogo className='h-4 w-4' />
            Airhouse
          </span>
        }
        description='Connect any Postgres-compatible client to your Airhouse-managed database.'
        actions={<AirhouseVersionBadge />}
      />
      <div className='space-y-6'>{renderContent()}</div>
    </div>
  );
};

export default Airhouse;
