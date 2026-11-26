import { isAxiosError } from "axios";
import { AppWindow, ExternalLink, Loader2, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { AppAccessBadge, AppAccessDialog } from "@/components/appAccess";
import { Button } from "@/components/ui/shadcn/button";
import { usePartnerOwnApps } from "@/hooks/api/appAccess";
import { lifecycleVerdict } from "@/pages/admin/AdminCustomApps/appStatus";
import { LifecycleVerdictPill } from "@/pages/admin/AdminCustomApps/components/AppLifecycle";
import type { AppAccessSummary } from "@/types/appAccess";

/**
 * The partner's OWN custom apps.
 *
 * A partner is a real org that uses Oxy itself, so it has apps of its own — but it
 * is not one of its own clients, so they never appeared in this console. These are
 * reached through `/partners/{id}/own-*`, authorized by **org authority** (an
 * officer of the partner org) rather than the partner ceiling: routing them through
 * the ceiling would hand every `manage_apps` operator control of their own org's
 * apps whether or not they are an officer of it.
 *
 * A non-officer operator therefore gets a 403 here and the panel simply doesn't
 * render — which is correct, not a bug.
 */
export default function OwnAppsPanel({
  partnerId,
  orgSlug
}: {
  partnerId: string;
  orgSlug: string;
}) {
  const { data: apps, isLoading, error } = usePartnerOwnApps(partnerId);
  const [managing, setManaging] = useState<AppAccessSummary | null>(null);

  // A 403 means "you're an operator but not an officer of your own org" — an
  // expected configuration, so stay quiet. Any OTHER failure (500, dropped
  // connection) must surface: silently vanishing would make a broken panel
  // indistinguishable from one that's correctly hidden.
  const notAnOfficer = isAxiosError(error) && error.response?.status === 403;
  if (notAnOfficer) return null;
  if (!isLoading && !error && !apps?.length) return null;

  return (
    <section className='border'>
      <header className='flex items-center gap-2 border-b px-3 py-2'>
        <AppWindow className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
        <h3 className='font-medium text-xs'>Your own apps</h3>
        <span className='text-[11px] text-muted-foreground'>{orgSlug}</span>
      </header>

      {error ? (
        <p className='px-3 py-3 text-[11px] text-destructive'>
          Couldn't load your apps. Reload to try again.
        </p>
      ) : isLoading ? (
        <div className='flex items-center gap-2 px-3 py-3 text-[11px] text-muted-foreground'>
          <Loader2 className='size-3 animate-spin' aria-hidden />
          Loading
        </div>
      ) : (
        <ul className='divide-y'>
          {(apps ?? []).map((app) => (
            <li
              key={app.id}
              className='flex items-center gap-2 px-3 py-1.5'
              data-testid={`partner-own-app-${app.slug}`}
            >
              <div className='min-w-0 flex-1'>
                <p className='truncate font-medium text-xs'>{app.name}</p>
                <p className='truncate text-[11px] text-muted-foreground'>{app.slug}</p>
              </div>
              {/* The same verdict word every other app list uses. Read-only here: the
                  own-apps routes carry no publish verb, so no card. */}
              <LifecycleVerdictPill
                verdict={lifecycleVerdict({ published: app.published })}
                compact
                className='text-[11px]'
                data-testid={`partner-own-app-verdict-${app.slug}`}
              />
              <AppAccessBadge visibility={app.visibility} grantCount={app.grant_count} />
              <Button
                variant='ghost'
                size='sm'
                className='h-6 gap-1 px-1.5 text-[11px]'
                onClick={() => setManaging(app)}
                data-testid={`partner-own-app-access-${app.slug}`}
              >
                <ShieldCheck className='size-3' aria-hidden />
                Access
              </Button>
              <Button asChild variant='ghost' size='sm' className='h-6 gap-1 px-1.5 text-[11px]'>
                <a
                  href={`/customer-apps/${orgSlug}/${app.slug}/`}
                  target='_blank'
                  rel='noreferrer'
                  data-testid={`partner-own-app-open-${app.slug}`}
                >
                  <ExternalLink className='size-3' aria-hidden />
                  Open
                </a>
              </Button>
            </li>
          ))}
        </ul>
      )}

      {managing && (
        <AppAccessDialog
          open
          onOpenChange={(next) => !next && setManaging(null)}
          scope={{ kind: "partner-own", partnerId }}
          appId={managing.id}
          appName={managing.name}
        />
      )}
    </section>
  );
}
