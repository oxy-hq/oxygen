import {
  AppWindow,
  Boxes,
  Factory,
  Home,
  Lock,
  ShieldCheck,
  SlidersHorizontal
} from "lucide-react";
import type { ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { CopyableId } from "@/pages/admin/components/CopyableId";
import type { CustomApp, OxyAccessRow } from "@/types/apps";
import { openWorkspaceHome } from "../../openWorkspaceHome";
import type { AccessOrg } from "./accessModel";
import { CopyPublishAction, formatGrantedAt, RowAction } from "./shared";

/**
 * Right pane of the Access browser. Oxy staff reach every workspace by default,
 * so this lists them ALL and flags the ones the org has locked us out of — a
 * lockdown is the exception worth seeing, and it's why an app won't open.
 */
export const OrgAccessDetail = ({
  org,
  apps,
  highlight
}: {
  org: AccessOrg;
  apps: CustomApp[];
  highlight: string;
}) => (
  <div className='flex h-full flex-col overflow-auto'>
    <header className='sticky top-0 z-10 flex items-center gap-2.5 border-b bg-background px-4 py-2.5'>
      <div className='flex size-7 shrink-0 items-center justify-center rounded-md border bg-muted/40'>
        {org.hasLockdown ? (
          <Lock className='size-3.5 text-destructive' />
        ) : (
          <ShieldCheck className='size-3.5 text-muted-foreground' />
        )}
      </div>
      <div className='min-w-0'>
        <h2 className='truncate font-semibold text-xs leading-tight'>{org.orgName}</h2>
        <p className='flex items-center gap-1 font-mono text-[11px] text-muted-foreground'>
          <span className='truncate'>{org.orgSlug}</span>
          <span aria-hidden>·</span>
          <CopyableId value={org.orgId} />
        </p>
      </div>
      <span className='ml-auto shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums'>
        {org.lockedCount > 0
          ? `${org.lockedCount} locked · ${org.accessibleCount} open`
          : `${org.accessibleCount} open`}
      </span>
    </header>

    <AppsSection apps={apps} />

    <SectionLabel>
      Workspaces
      <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>
        {org.workspaces.length}
      </span>
    </SectionLabel>
    {org.workspaces.length === 0 ? (
      <EmptyBody workspaceCount={org.workspaceCount} />
    ) : (
      <ul className='flex flex-col divide-y divide-border/50'>
        {org.workspaces.map((w) => (
          <WorkspaceAccessRow
            key={w.workspace_id}
            row={w}
            dim={!!highlight && !w.workspace_name.toLowerCase().includes(highlight)}
          />
        ))}
      </ul>
    )}
  </div>
);

const SectionLabel = ({ children }: { children: ReactNode }) => (
  <div className='flex items-center justify-between border-b bg-muted/20 px-4 py-1.5 font-medium text-[11px] text-muted-foreground uppercase tracking-wider'>
    {children}
  </div>
);

/** The org's custom apps — each links straight to its full detail cockpit. */
const AppsSection = ({ apps }: { apps: CustomApp[] }) => {
  const navigate = useNavigate();
  return (
    <>
      <SectionLabel>
        Apps
        <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>
          {apps.length}
        </span>
      </SectionLabel>
      {apps.length === 0 ? (
        <p className='border-b px-4 py-2.5 text-muted-foreground text-xs'>
          No custom apps in this organization.
        </p>
      ) : (
        <ul className='flex flex-col divide-y divide-border/50 border-b'>
          {apps.map((a) => (
            <li key={a.id}>
              <button
                type='button'
                onClick={() => navigate(`/admin/apps/${a.org_slug}/${a.slug}`)}
                className='flex w-full items-center gap-2.5 px-4 py-2 text-left transition-colors hover:bg-muted/40'
              >
                <AppWindow className='size-3.5 shrink-0 text-muted-foreground/60' />
                <span className='min-w-0 flex-1 truncate font-medium text-xs'>{a.name}</span>
                <span className='shrink-0 truncate font-mono text-[11px] text-muted-foreground'>
                  {a.slug}
                </span>
                <Badge
                  variant={a.published_at ? "secondary" : "outline"}
                  className='shrink-0 px-1.5 py-0 text-[10px]'
                >
                  {a.published_at ? "Published" : "Unpublished"}
                </Badge>
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );
};

const EmptyBody = ({ workspaceCount }: { workspaceCount: number }) => (
  <div className='flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center'>
    <div className='flex size-10 items-center justify-center rounded-full border bg-muted/30'>
      <Boxes className='size-3.5 text-muted-foreground' />
    </div>
    <p className='font-medium text-xs'>No workspaces</p>
    <p className='max-w-xs text-muted-foreground text-xs'>
      This org has {workspaceCount} workspace{workspaceCount === 1 ? "" : "s"} in the directory but
      none resolved here.
    </p>
  </div>
);

const WorkspaceAccessRow = ({ row, dim }: { row: OxyAccessRow; dim?: boolean }) => {
  const navigate = useNavigate();
  return (
    <li className={cn("flex items-center gap-2.5 px-4 py-2", dim && "opacity-40")}>
      {row.locked ? (
        <Lock className='size-3.5 shrink-0 text-destructive' />
      ) : (
        <Boxes className='size-3.5 shrink-0 text-muted-foreground/60' />
      )}
      <div className='min-w-0 flex-1'>
        <div className='flex items-center gap-2'>
          <span className='truncate font-medium text-xs'>{row.workspace_name}</span>
          <CopyableId value={row.workspace_id} className='shrink-0' />
          {row.locked && (
            <Badge variant='destructive' className='shrink-0 px-1.5 py-0 text-[10px]'>
              Locked out
            </Badge>
          )}
        </div>
        {/* Only the locked case says anything — "open" is the default for every
            other workspace, so a per-row "open to Oxy staff" was just noise. */}
        {row.locked && (
          <p className='truncate font-mono text-[11px] text-muted-foreground'>
            {`locked ${row.locked_at ? formatGrantedAt(row.locked_at) : ""}${
              row.locked_by_email ? ` · ${row.locked_by_email}` : ""
            }`}
          </p>
        )}
      </div>
      <div className='flex shrink-0 items-center gap-0.5'>
        <CopyPublishAction workspaceId={row.workspace_id} />
        {/* A locked workspace stays inspectable in admin, but its apps won't open
            for staff — so the app-facing actions are disabled, not hidden. */}
        <RowAction
          icon={Factory}
          label={row.locked ? "Locked out by the org" : "Open in Oxygen Nexus (IDE)"}
          disabled={row.locked}
          onClick={() =>
            window.open(
              ROUTES.ORG(row.org_slug).ENTER_WORKSPACE(row.workspace_id).IDE.ROOT,
              "_blank",
              "noopener"
            )
          }
        />
        <RowAction
          icon={SlidersHorizontal}
          label='Manage workspace'
          onClick={() => navigate(ROUTES.ADMIN.WORKSPACE_DETAIL(row.workspace_id))}
        />
        <RowAction
          icon={Home}
          label={row.locked ? "Locked out by the org" : "Open /home (new tab)"}
          disabled={row.locked}
          onClick={() => openWorkspaceHome(row)}
        />
      </div>
    </li>
  );
};
