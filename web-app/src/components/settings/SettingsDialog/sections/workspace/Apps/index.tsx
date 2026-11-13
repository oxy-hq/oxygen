import { AppWindow } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import WorkspaceRepositoryNotice from "@/components/WorkspaceRepositoryNotice";
import { useAppIntegrations } from "@/hooks/api/appIntegrations/useAppIntegrations";
import type { AppIntegration, AppIntegrationKind } from "@/services/api/appIntegrations";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import { AppCard } from "./components/AppCard";
import { ToastWebhookSetup } from "./components/ToastWebhookSetup";

const APP_KINDS: AppIntegrationKind[] = ["toast", "openweathermap", "besttime", "unifi"];

/**
 * The workspace's app integrations, read-only. An integration is declared in
 * `config.yml`, so connecting or changing one is a change to the workspace's
 * repository.
 */
const Apps: React.FC = () => {
  const { data } = useAppIntegrations();
  const { workspace } = useCurrentWorkspace();

  const byKind = useMemo(() => {
    const map = new Map<AppIntegrationKind, AppIntegration>();
    for (const entry of data ?? []) map.set(entry.kind, entry);
    return map;
  }, [data]);

  return (
    <CanWorkspaceAdmin
      fallback={<NoAccessNotice>You need workspace admin access to manage apps.</NoAccessNotice>}
    >
      <div className='flex flex-col gap-5'>
        <SectionHeader
          icon={AppWindow}
          title='Apps'
          description='External services that feed downstream dashboards. Credentials live in the workspace secrets store and are referenced from config.yml.'
        />

        <WorkspaceRepositoryNotice />

        <div className='grid grid-cols-1 gap-3 md:grid-cols-2'>
          {APP_KINDS.map((kind) => (
            <AppCard
              key={kind}
              kind={kind}
              integration={byKind.get(kind) ?? null}
              footer={
                kind === "toast" ? <ToastWebhookSetup projectId={workspace?.id ?? ""} /> : null
              }
            />
          ))}
        </div>
      </div>
    </CanWorkspaceAdmin>
  );
};

export default Apps;
