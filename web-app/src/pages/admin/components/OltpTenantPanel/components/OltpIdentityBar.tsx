import { ExternalLink } from "lucide-react";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { OltpConnectionInfo } from "@/services/api/oltp";

/**
 * Everything that identifies the database, on one line.
 *
 * This was a four-cell grid over a separate row of pills — two bands for one
 * line's worth of facts — and the grid clipped: `oxy_org_00000000_…` ran
 * straight into the host beside it, because a fixed 4-up column is narrower
 * than a tenant database name. Here the identifiers flow with `min-w-0` +
 * `truncate`, so a long name shortens instead of colliding, and the status
 * chips sit right-aligned where a scan for trouble ends.
 */
export const OltpIdentityBar = ({ data }: { data: OltpConnectionInfo }) => {
  const drifted = data.platform_schema_version !== data.expected_platform_schema_version;
  return (
    <div
      className='flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-border-subtle border-b pb-2'
      data-testid='admin-org-oltp-identity'
    >
      <div className='flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-xs'>
        <span className='truncate font-medium' title={data.database}>
          {data.database}
        </span>
        <span className='text-muted-foreground'>·</span>
        <span className='truncate text-muted-foreground' title={data.host}>
          {data.host}
        </span>
        <span className='text-muted-foreground'>·</span>
        {data.console_url ? (
          // The project's console, one click from the identity line — the usual
          // next step when an operator is looking at a provisioned tenant. The
          // title carries the project name; the segment stays compact.
          <a
            href={data.console_url}
            target='_blank'
            rel='noopener noreferrer'
            title={data.project_name}
            // The visible text is `neon/region`; the project name is the
            // accessible name, since that is what the link actually opens.
            aria-label={`Open ${data.project_name} in the provider console`}
            className='inline-flex items-center gap-1 text-primary hover:underline'
          >
            {data.provider}
            {data.region ? `/${data.region}` : ""}
            <ExternalLink className='h-3 w-3 shrink-0' aria-hidden />
          </a>
        ) : (
          <span className='text-muted-foreground' title={data.project_name || undefined}>
            {data.provider}
            {data.region ? `/${data.region}` : ""}
          </span>
        )}
      </div>
      <div className='flex shrink-0 items-center gap-1.5'>
        <AdminStatusPill tone={data.is_provisioned ? "ok" : "warn"} label={data.status} />
        <AdminStatusPill
          tone={data.analyst_ready ? "ok" : "danger"}
          label={data.analyst_ready ? "analyst ready" : "analyst NOT minted"}
        />
        <AdminStatusPill
          tone={drifted ? "warn" : "muted"}
          label={`platform v${data.platform_schema_version}/${data.expected_platform_schema_version}`}
        />
      </div>
    </div>
  );
};
