import { AppMark } from "@/components/apps/AppMark";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Table, TableBody, TableCell, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { type PartnerAppWithOrg, usePartnerApps, usePartnerOrgs } from "@/hooks/api/partners";
import { cn } from "@/libs/shadcn/utils";
import { AgentSkillsButton } from "@/pages/admin/AdminCustomApps/components/AgentSkills";
import { PartnerAppLifecycle } from "@/pages/admin/AdminCustomApps/components/AppLifecycle";
import CiInstructions from "@/pages/admin/AdminTokens/components/CiInstructions";
import {
  ADMIN_HEADER_ROW_CLASS,
  ADMIN_ROW_CLASS,
  AdminTh
} from "@/pages/admin/components/AdminTable";
import OwnAppsPanel from "../components/OwnAppsPanel";
import PageShell from "../components/PageShell";
import { usePartnerConsole } from "../context";
import AppTokenManager from "./components/AppTokenManager";

/**
 * Custom apps — the one place a partner sees every app it ships to clients and
 * how to deploy them. Same visual language as the admin customer-apps console
 * (fleet strip + dense table with app marks and status LEDs), scoped to the
 * orgs this partner manages. Gated by `manage_apps`; the server re-checks
 * every mutation.
 */
export default function PartnerCustomApps() {
  const { active } = usePartnerConsole();

  return (
    <PageShell
      eyebrow='Your clients'
      title='Custom apps'
      description='Every app you ship to your clients — published state at a glance, plus how to deploy from CI.'
      testId='partner-custom-apps-page'
      actions={<AgentSkillsButton />}
    >
      {/* The partner's OWN apps sit above the client fleet and OUTSIDE the
          manage_apps gate: they're reached by org authority, not the ceiling, so an
          operator without manage_apps can still administer their own org's apps if
          they're an officer of it. The panel hides itself when they aren't. */}
      <div className='space-y-8'>
        <OwnAppsPanel partnerId={active.partner_id} orgSlug={active.slug} />

        {active.capabilities.manage_apps ? (
          <>
            <AppsSection partnerId={active.partner_id} />
            <PublishingSection partnerId={active.partner_id} />
          </>
        ) : (
          <p className='text-muted-foreground text-sm'>
            Your partnership isn't granted app management (the <code>manage_apps</code> ceiling), so
            there's nothing here for your clients yet.
          </p>
        )}
      </div>
    </PageShell>
  );
}

/** Every managed app across every client — a fleet strip over a dense table. */
function AppsSection({ partnerId }: { partnerId: string }) {
  const { data: orgs } = usePartnerOrgs(partnerId);
  const { apps, isLoading } = usePartnerApps(partnerId, orgs);

  if (isLoading) return <Skeleton className='h-40 w-full rounded-lg' />;

  if (apps.length === 0)
    return (
      <div className='rounded-lg border border-dashed p-8 text-center text-muted-foreground text-sm'>
        No custom apps across your clients yet.
      </div>
    );

  return (
    <div className='overflow-hidden rounded-lg border'>
      <FleetStrip apps={apps} clientCount={orgs?.length ?? 0} />
      <Table>
        <TableHeader>
          <TableRow className={ADMIN_HEADER_ROW_CLASS}>
            <AdminTh>App</AdminTh>
            <AdminTh>Client</AdminTh>
            <AdminTh>Lifecycle</AdminTh>
          </TableRow>
        </TableHeader>
        <TableBody>
          {apps.map((a) => (
            <TableRow
              key={a.id}
              className={ADMIN_ROW_CLASS}
              data-testid={`partner-apps-row-${a.orgSlug}-${a.slug}`}
            >
              <TableCell>
                <div className='flex items-center gap-2'>
                  <AppMark name={a.name} size='sm' />
                  <div className='min-w-0'>
                    <div className='max-w-[26ch] truncate font-medium text-sm'>{a.name}</div>
                    <div className='truncate font-mono text-[11px] text-muted-foreground'>
                      {a.slug}
                    </div>
                  </div>
                </div>
              </TableCell>
              <TableCell className='max-w-[160px] truncate text-muted-foreground text-sm'>
                {a.orgName}
              </TableCell>
              <TableCell>
                {/* The same lifecycle card the staff console shows, in its row form:
                    the verdict and the one verb that applies (Publish / Unpublish),
                    behind the same confirm. The Switch it replaces flipped the live
                    pointer with no confirmation and no shared words. */}
                <PartnerAppLifecycle
                  app={a}
                  partnerId={partnerId}
                  orgId={a.orgId}
                  orgSlug={a.orgSlug}
                  variant='row'
                />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

/** Summary bar over the table — mirrors the admin console's fleet strip. */
function FleetStrip({ apps, clientCount }: { apps: PartnerAppWithOrg[]; clientCount: number }) {
  const live = apps.filter((a) => a.published).length;
  return (
    <div className='flex flex-wrap items-center gap-x-5 gap-y-1.5 border-b bg-muted/20 px-4 py-2 text-xs'>
      <FleetStat value={apps.length} label='apps' />
      <FleetStat value={live} label='live' led='live' />
      <FleetStat value={apps.length - live} label='draft' led='draft' />
      <FleetStat value={clientCount} label={clientCount === 1 ? "client" : "clients"} />
    </div>
  );
}

function FleetStat({
  value,
  label,
  led
}: {
  value: number;
  label: string;
  led?: "live" | "draft";
}) {
  return (
    <span className='flex items-center gap-1.5'>
      {led ? <StatusDot isLive={led === "live"} /> : null}
      <span className='font-semibold text-foreground text-sm tabular-nums'>{value}</span>
      <span className='text-muted-foreground'>{label}</span>
    </span>
  );
}

function StatusDot({ isLive }: { isLive: boolean }) {
  return (
    <span
      aria-hidden
      className={cn(
        "size-2 shrink-0 rounded-full",
        isLive ? "bg-primary ring-2 ring-primary/25" : "border border-muted-foreground/50"
      )}
    />
  );
}

/** Trusted-publishing setup — CI how-to + the partner's app-scoped tokens. */
function PublishingSection({ partnerId }: { partnerId: string }) {
  return (
    <section className='space-y-3'>
      <div>
        <h2 className='font-semibold text-sm'>Publishing from CI</h2>
        <p className='text-muted-foreground text-xs'>
          Ship updates with trusted publishing — GitHub Actions mints a short-lived, app-scoped
          credential per run, so there's no stored secret.
        </p>
      </div>
      <CiInstructions showTokenPath={false} />
      <AppTokenManager partnerId={partnerId} />
    </section>
  );
}
