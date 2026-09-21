import { ShieldCheck } from "lucide-react";
import { useState } from "react";
import { AppAccessBadge, AppAccessDialog } from "@/components/appAccess";
import { Button } from "@/components/ui/shadcn/button";
import { useAppAccess } from "@/hooks/api/appAccess";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { CustomApp } from "@/types/apps";

/**
 * Who can open this app, inside the admin dossier.
 *
 * Staff edit through `/admin/apps/{id}/access` rather than the org's own route:
 * org routes need a live assume-role session, and `block_admin_while_acting`
 * closes this whole console the moment an operator starts one. Same service behind
 * both, so what an operator sees here is exactly what the org's admins see.
 */
export function AppAccessPane({ app }: { app: CustomApp }) {
  const [editing, setEditing] = useState(false);
  const access = useAppAccess({ kind: "admin" }, app.id);

  return (
    <div className='flex flex-col gap-3 p-4 pt-0'>
      {/* The failure used to render in `text-muted-foreground`, which reads as "nothing
          to show" — the one thing an access pane must never say when it does not know.
          It also offered no retry. */}
      <AdminAsync query={access} noun='access settings' rows={1}>
        {(data) => {
          const teamGrants = data.grants.filter((g) => g.kind === "team");
          const userGrants = data.grants.filter((g) => g.kind === "user");
          return (
            <>
              <div className='flex items-center gap-2'>
                <AppAccessBadge visibility={data.visibility} grantCount={data.grants.length} />
                <Button
                  variant='ghost'
                  size='sm'
                  className='ml-auto h-7'
                  onClick={() => setEditing(true)}
                >
                  <ShieldCheck className='size-3.5' aria-hidden />
                  Manage
                </Button>
              </div>

              {data.visibility === "members" && (
                <dl className='grid gap-1.5 text-xs'>
                  <div className='flex gap-2'>
                    <dt className='w-16 shrink-0 text-muted-foreground'>Teams</dt>
                    <dd className='min-w-0 flex-1'>
                      {teamGrants.length === 0 ? (
                        <span className='text-muted-foreground'>none</span>
                      ) : (
                        teamGrants
                          .map((g) => `${g.name}${g.role === "admin" ? " (admin)" : ""}`)
                          .join(", ")
                      )}
                    </dd>
                  </div>
                  <div className='flex gap-2'>
                    <dt className='w-16 shrink-0 text-muted-foreground'>People</dt>
                    <dd className='min-w-0 flex-1'>
                      {userGrants.length === 0 ? (
                        <span className='text-muted-foreground'>none</span>
                      ) : (
                        userGrants
                          .map((g) => `${g.name}${g.role === "admin" ? " (admin)" : ""}`)
                          .join(", ")
                      )}
                    </dd>
                  </div>
                  {data.grants.length === 0 && (
                    <p className='pt-1 text-muted-foreground leading-relaxed'>
                      Restricted with nothing granted — only the org's owners and admins can open
                      it.
                    </p>
                  )}
                </dl>
              )}
            </>
          );
        }}
      </AdminAsync>

      {/* Mounted only while open: the dialog's own queries (teams, people) key off
          `appId`, and rendering it unconditionally fired them on every dossier
          render for a picker nobody had opened. */}
      {editing && (
        <AppAccessDialog
          open
          onOpenChange={setEditing}
          scope={{ kind: "admin" }}
          appId={app.id}
          appName={app.name}
        />
      )}
    </div>
  );
}
