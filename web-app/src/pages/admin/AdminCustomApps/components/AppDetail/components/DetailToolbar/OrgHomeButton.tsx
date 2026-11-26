import { Home } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { useAdminOrgSubdomain } from "@/hooks/api/adminTenants";
import type { CustomApp } from "@/types/apps";

/**
 * "Org home" — the org's HQ launcher on its own subdomain.
 *
 * Sits beside **Act as org** because the two answer adjacent questions and get
 * confused: this one *looks at* the org's launcher (does the app card appear? is it
 * restricted?), while Act-as *becomes* the org (real data, audited, closes admin for
 * 60 minutes). Grouping them makes the cheap check the obvious first move.
 *
 * **The URL comes from the server, not from `window.location`.** Deriving it here by
 * swapping the leftmost hostname label would be a second, weaker copy of
 * `org_subdomain_zone()` and would be wrong in the two ways that matter: the zone is
 * `OXY_ORG_SUBDOMAIN_ZONE` (only derived from the API URL when the admin host's
 * first label is exactly `app`), so from `app-staging.oxygen-hq.com` the guess would
 * point at the PRODUCTION org host — a confident answer about the wrong database.
 * And org subdomains are **opt-in per org**: without an enabled `org_subdomains`
 * row, the host 302s to the app root, so the button would silently land the operator
 * on their own home while claiming to show the org's.
 *
 * So: render only when the server says the org has one enabled.
 */
export function OrgHomeButton({ app }: { app: CustomApp }) {
  const { data: subdomain } = useAdminOrgSubdomain(app.org_id);

  // No subdomain, not enabled, or no derivable zone → nothing to link to. Render
  // nothing rather than a button that goes somewhere misleading.
  if (!subdomain?.enabled || !subdomain.url) return null;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          asChild
          variant='outline'
          size='sm'
          className='h-6 gap-1 px-1.5 text-[11px]'
          data-testid='admin-app-org-home'
        >
          <a href={subdomain.url} target='_blank' rel='noreferrer'>
            <Home className='size-3' />
            Org home
          </a>
        </Button>
      </TooltipTrigger>
      <TooltipContent className={`${DESIGN_REFERENCE} max-w-xs space-y-1.5`}>
        <p className='font-medium'>Open {app.org_slug}'s home page</p>
        <p>
          The org's launcher at <code>{subdomain.url}</code>, in a new tab — where its apps actually
          appear. Use this to check whether a card shows up before starting an acting session.
        </p>
        <p className='text-muted-foreground'>
          Opens as you, so a restricted app you hold no grant on won't be listed.
        </p>
      </TooltipContent>
    </Tooltip>
  );
}
