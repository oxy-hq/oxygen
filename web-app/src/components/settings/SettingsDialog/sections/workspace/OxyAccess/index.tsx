import { Globe, Loader2, ShieldOff } from "lucide-react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Switch } from "@/components/ui/shadcn/switch";
import { useOrgSubdomain } from "@/hooks/api/access/useOrgSubdomain";
import { useOxyAccess, useSetOxyLockdown } from "@/hooks/api/access/useOxyAccess";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

function formatAt(value: string): string {
  return new Date(value).toLocaleString();
}

/**
 * Workspace setting: **Oxy staff access**.
 *
 * Inverted from the old opt-in consent toggle. Oxy support can reach this
 * workspace's apps by default — no setup, no friction. This switch is the
 * customer's kill switch: turning it on LOCKS Oxy staff out.
 *
 * `can_manage` comes from the server and is the authority: only a real org
 * owner/admin may flip it. An Oxy operator viewing this workspace sees the
 * state but cannot change it — they must not be able to unlock themselves.
 */
export default function OxyAccess() {
  const { workspace } = useCurrentWorkspace();
  const workspaceId = workspace?.id ?? "";

  const { data: status, isPending } = useOxyAccess(workspaceId);
  const setLockdown = useSetOxyLockdown(workspaceId);
  const { data: subdomain } = useOrgSubdomain(workspaceId);

  if (!workspace) return null;

  const locked = status?.locked ?? false;
  const canManage = status?.can_manage ?? false;

  return (
    <div className='flex flex-col gap-6'>
      <div className='flex items-start gap-4 rounded-lg border bg-card p-5'>
        <div
          className={
            locked
              ? "flex size-10 shrink-0 items-center justify-center rounded-md border bg-destructive/10 text-destructive"
              : "flex size-10 shrink-0 items-center justify-center rounded-md border bg-primary/10 text-primary"
          }
        >
          <ShieldOff className='size-5' />
        </div>

        <div className='flex flex-1 flex-col gap-1'>
          <p className='font-medium text-sm leading-tight'>Lock Oxy staff out of this workspace</p>
          {isPending ? (
            <Skeleton className='mt-1 h-3.5 w-48' />
          ) : locked ? (
            <p className='text-muted-foreground text-xs'>
              Locked{status?.locked_at ? ` since ${formatAt(status.locked_at)}` : ""} — Oxy support
              cannot see this workspace's apps.
            </p>
          ) : (
            <p className='text-muted-foreground text-xs'>
              Oxy support can open this workspace's apps to help you.
            </p>
          )}
        </div>

        <div className='flex items-center gap-3'>
          {setLockdown.isPending && (
            <Loader2 className='size-3.5 animate-spin text-muted-foreground' />
          )}
          <Switch
            checked={locked}
            disabled={!canManage || isPending || setLockdown.isPending}
            onCheckedChange={(next) => setLockdown.mutate(next)}
            aria-label='Lock Oxy staff out of this workspace'
          />
        </div>
      </div>

      {/* Org subdomain — read-only. It's an Oxy-managed capability (enabled by
          Oxy staff for select orgs), so the customer sees status only, no
          toggle. Shown only when live. */}
      {subdomain?.enabled && subdomain.url && (
        <div className='flex items-start gap-4 rounded-lg border bg-card p-5'>
          <div className='flex size-10 shrink-0 items-center justify-center rounded-md border bg-primary/10 text-primary'>
            <Globe className='size-5' />
          </div>
          <div className='flex flex-1 flex-col gap-1'>
            <p className='font-medium text-sm leading-tight'>Served on its own subdomain</p>
            <p className='text-muted-foreground text-xs'>
              Live at <span className='font-mono'>{subdomain.url}</span>
              {subdomain.is_default_workspace ? " — this workspace is the default project" : ""}
            </p>
          </div>
        </div>
      )}

      {!isPending && !canManage && (
        <p className='text-muted-foreground text-xs'>
          Only this organization's owners and admins can change this — Oxy staff cannot.
        </p>
      )}
    </div>
  );
}
