import { AppWindow, Loader2 } from "lucide-react";
import { useState } from "react";
import { AppAccessBadge, AppAccessDialog } from "@/components/appAccess";
import { Button } from "@/components/ui/shadcn/button";
import { useOrgAppAccessList } from "@/hooks/api/appAccess";
import type { AppAccessSummary } from "@/types/appAccess";
import type { Organization, OrgRole } from "@/types/organization";

/**
 * Who can open each of the org's custom apps.
 *
 * Separate from Teams because they answer different questions: Teams is "who works
 * together", this is "what can they reach". An admin usually arrives here from a
 * question about one app, so the app is the row and access is the action.
 */
export default function AppAccessSection({
  org,
  viewerRole
}: {
  org: Organization;
  viewerRole: OrgRole;
}) {
  const orgId = org.id;
  const canManage = viewerRole === "owner" || viewerRole === "admin";
  const [editing, setEditing] = useState<AppAccessSummary | null>(null);

  const { data: apps, isPending, isError } = useOrgAppAccessList(orgId, canManage);

  if (!canManage) {
    return (
      <div className='flex items-center justify-center py-12'>
        <p className='text-muted-foreground text-sm'>
          You need to be an organization owner or admin to change who can open an app.
        </p>
      </div>
    );
  }

  return (
    <div className='flex flex-col gap-5'>
      {isPending ? (
        <div className='flex items-center justify-center py-12'>
          <Loader2 className='size-5 animate-spin text-muted-foreground' aria-hidden />
          <span className='sr-only'>Loading apps</span>
        </div>
      ) : isError ? (
        <p className='py-12 text-center text-muted-foreground text-sm'>
          Couldn't load apps. Reopen this page to try again.
        </p>
      ) : (apps ?? []).length === 0 ? (
        <div className='flex flex-col items-center gap-2 rounded-lg border border-dashed px-6 py-12 text-center'>
          <AppWindow className='size-6 text-muted-foreground' aria-hidden />
          <p className='font-medium text-sm'>No custom apps yet</p>
          <p className='max-w-sm text-muted-foreground text-xs leading-relaxed'>
            Once an app is published to this organization it shows up here, and you can decide who
            sees it.
          </p>
        </div>
      ) : (
        <ul className='divide-y rounded-lg border'>
          {(apps ?? []).map((app) => (
            <li key={app.id} className='flex items-center gap-3 px-4 py-3'>
              <AppWindow className='size-4 shrink-0 text-muted-foreground' aria-hidden />
              <div className='min-w-0 flex-1'>
                <p className='truncate font-medium text-sm'>{app.name}</p>
                <p className='truncate text-muted-foreground text-xs'>
                  {app.published ? app.slug : `${app.slug} · not published`}
                </p>
              </div>
              <AppAccessBadge visibility={app.visibility} grantCount={app.grant_count} />
              <Button variant='ghost' size='sm' onClick={() => setEditing(app)}>
                Manage
              </Button>
            </li>
          ))}
        </ul>
      )}

      <AppAccessDialog
        open={editing !== null}
        onOpenChange={(open) => !open && setEditing(null)}
        scope={{ kind: "org", orgId }}
        appId={editing?.id ?? null}
        appName={editing?.name ?? ""}
      />
    </div>
  );
}
