import { Database, ExternalLink } from "lucide-react";
import type React from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import useOltpConnection from "@/hooks/api/oltp/useOltpConnection";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import SectionHeader from "../../../components/SectionHeader";
import SchemaDiagram from "./components/SchemaDiagram";

/** A labelled read-only value, matching the Airhouse connection panel. */
const Field: React.FC<{ label: string; value: string }> = ({ label, value }) => (
  <div className='flex flex-col gap-1'>
    <span className='text-muted-foreground text-xs'>{label}</span>
    <span className='break-all font-mono text-sm'>{value}</span>
  </div>
);

/**
 * The project field. A plain value on local/mock; a link to the provider
 * console when there is one, since opening the project is the operator's usual
 * next step after seeing it is provisioned.
 */
const ProjectField: React.FC<{ name: string; url: string | null }> = ({ name, url }) => (
  <div className='flex flex-col gap-1'>
    <span className='text-muted-foreground text-xs'>Project</span>
    {url ? (
      <a
        href={url}
        target='_blank'
        rel='noopener noreferrer'
        className='inline-flex items-center gap-1 break-all font-mono text-primary text-sm hover:underline'
      >
        {name}
        <ExternalLink className='h-3 w-3 shrink-0' aria-hidden />
      </a>
    ) : (
      <span className='break-all font-mono text-sm'>{name}</span>
    )}
  </div>
);

const Oltp: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: connection, isLoading, error } = useOltpConnection(workspace?.id);
  const showConfigSnippet = !useAppsFirstExperience() || canUseFactory(workspace);

  const renderContent = () => {
    if (isLoading) {
      return <p className='text-muted-foreground text-sm'>Loading…</p>;
    }
    if (error) {
      return (
        <p className='text-muted-foreground text-sm'>
          Couldn't load the OLTP database status:{" "}
          {error instanceof Error ? error.message : "unknown error"}
        </p>
      );
    }
    if (!connection?.is_provisioned) {
      // Provisioning is an operator action, not a workspace-settings one, so
      // this points at where it happens rather than offering a button that
      // would only fail for a non-admin. (It used to name a raw
      // `cargo run --example seed_org`, from before the admin console had a
      // Provision button.)
      return (
        <div className='flex flex-col gap-3'>
          <p className='text-muted-foreground text-sm'>
            This organization doesn't have an OLTP database yet. It gives custom apps and Airway
            pipelines a place to write transactional data — orders, bookings, records — that the
            warehouse isn't built for.
          </p>
          <div className='rounded-md border bg-muted/40 p-3'>
            <p className='text-muted-foreground text-xs'>
              An operator can provision one from the admin console (Orgs → this org → OLTP), or with{" "}
              <code className='font-mono text-xs'>oxy oltp provision</code>.
            </p>
          </div>
        </div>
      );
    }

    const behindOnPlatform =
      connection.platform_schema_version < connection.expected_platform_schema_version;

    return (
      <div className='flex flex-col gap-6'>
        <div className='grid grid-cols-2 gap-4'>
          <Field label='Database' value={connection.database} />
          <Field label='Host' value={connection.host} />
          <Field label='Provider' value={connection.provider} />
          <Field label='Region' value={connection.region} />
          <ProjectField name={connection.project_name} url={connection.console_url} />
        </div>

        <div className='flex flex-col gap-2'>
          <span className='text-muted-foreground text-xs'>Query access</span>
          <p className='text-sm'>
            Queries from the IDE and agents connect as{" "}
            <code className='font-mono text-xs'>{connection.analyst_role}</code>, which is
            read-only. Writes come only from published app functions and Airway pipelines.
          </p>
          {/* Configuring it is the Factory's; apps-first, only a Factory user sees how. */}
          {showConfigSnippet && (
            <div className='rounded-md border bg-muted/40 p-3'>
              <p className='mb-2 text-muted-foreground text-xs'>Add to config.yml to query it:</p>
              <pre className='font-mono text-xs'>
                {"databases:\n  - name: oltp\n    type: postgres_managed"}
              </pre>
            </div>
          )}
          {!connection.analyst_ready && (
            <p className='text-destructive text-sm'>
              The read-only credential hasn't been created yet, so{" "}
              <code className='font-mono text-xs'>postgres_managed</code> can't connect. Re-run
              provisioning to create it.
            </p>
          )}
        </div>

        <div className='flex flex-col gap-2'>
          <span className='text-muted-foreground text-xs'>Schemas</span>
          {connection.schemas.length === 0 ? (
            <p className='text-muted-foreground text-sm'>
              No apps or pipelines have written here yet. Each one gets its own schema the first
              time it runs.
            </p>
          ) : (
            <div className='flex flex-col divide-y rounded-md border'>
              {connection.schemas.map((s) => (
                <div key={s.schema} className='flex items-center justify-between gap-3 p-3'>
                  <div className='flex flex-col gap-0.5'>
                    <span className='font-mono text-sm'>{s.schema}</span>
                    <span className='text-muted-foreground text-xs'>
                      {s.kind === "app" ? "Custom app" : "Airway pipeline"} · {s.writer_name}
                    </span>
                  </div>
                  <Badge variant={s.analytics_visible ? "secondary" : "outline"}>
                    {s.analytics_visible ? "Visible to analytics" : "Hidden from analytics"}
                  </Badge>
                </div>
              ))}
            </div>
          )}
          <p className='text-muted-foreground text-xs'>
            Pipeline data is visible to analytics by default. An app's data stays hidden until the
            app opts in, because it holds live records.
          </p>
        </div>

        <div className='flex flex-col gap-2'>
          <span className='text-muted-foreground text-xs'>Schema diagram</span>
          <SchemaDiagram workspaceId={workspace?.id} />
        </div>

        {behindOnPlatform && (
          <p className='text-muted-foreground text-sm'>
            This database is on platform schema v{connection.platform_schema_version}; the current
            version is v{connection.expected_platform_schema_version}. It updates the next time
            anything connects.
          </p>
        )}
      </div>
    );
  };

  // Open to any org member: this panel returns no credentials, only which
  // schemas exist and whether analytics can see them.
  return (
    <div className='flex flex-col gap-5'>
      <SectionHeader
        title={
          <span className='flex items-center gap-2'>
            <Database className='h-4 w-4' />
            OLTP Database
          </span>
        }
        description='Where your custom apps and pipelines write transactional data.'
      />
      <div className='space-y-6'>{renderContent()}</div>
    </div>
  );
};

export default Oltp;
