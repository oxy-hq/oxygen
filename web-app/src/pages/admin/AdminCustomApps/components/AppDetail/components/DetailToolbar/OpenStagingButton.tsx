import { FlaskConical } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { CustomApp } from "@/types/apps";

/**
 * "Open staging" — the app's staging host
 * (`https://staging--<org>--<slug>.customer-apps.<zone>/`), serving the
 * unpromoted (draft) build with writes held or isolated. See "## Staging" in
 * `internal-docs/customer-apps-functions.md`.
 *
 * Before this, staff had to type the staging host by hand — the URL builder
 * (`environment_url_for`) existed but nothing in the admin UI linked to it.
 *
 * `app.staging_url` is computed server-side (`staging_url_for` in the admin
 * apps DTO, detail response only) and is only present when there's something
 * distinct to see there: render nothing otherwise — no staging build yet,
 * the staging build already matches what's live, or the zone can't be
 * derived (e.g. local dev, where `OXY_API_URL` doesn't fit the convention).
 */
export function OpenStagingButton({ app }: { app: CustomApp }) {
  if (!app.staging_url) return null;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          asChild
          variant='outline'
          size='sm'
          className='h-6 gap-1 px-1.5 text-[11px]'
          data-testid='admin-app-open-staging'
        >
          <a href={app.staging_url} target='_blank' rel='noreferrer'>
            <FlaskConical className='size-3' />
            Open staging
          </a>
        </Button>
      </TooltipTrigger>
      <TooltipContent className='max-w-xs space-y-1.5'>
        <p className='font-medium'>Open the staging build</p>
        <p>Runs the draft build on real data; writes are held or go to staging copies.</p>
      </TooltipContent>
    </Tooltip>
  );
}
