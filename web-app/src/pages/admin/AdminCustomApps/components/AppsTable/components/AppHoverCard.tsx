import { EyeOff, Rocket } from "lucide-react";
import type { ReactNode } from "react";
import { AppMark } from "@/components/apps/AppMark";
import { Button } from "@/components/ui/shadcn/button";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/shadcn/hover-card";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../../resolveBundleUrl";
import { formatRelativeTime } from "../useAppsTable";
import { StatusDot } from "./StatusDot";
import { CopyButton, OpenAppButton } from "./UrlActions";

interface AppHoverCardProps {
  app: CustomApp;
  /** Show the org slug row — hidden when the surface already groups by org. */
  showOrg?: boolean;
  onPublish: (app: CustomApp) => void;
  onUnpublish: (app: CustomApp) => void;
  /** Preferred side; Radix auto-flips on collision. Registry rows point right
   *  into the stage; gallery cards default right too and flip near the edge. */
  side?: "top" | "right" | "bottom" | "left";
  children: ReactNode;
}

/**
 * The reveal-on-hover dossier shared by the gallery card and the registry rail
 * row. Surfaces the operational facts that don't earn a spot on the dense
 * face — project, branch, recency, promoter — plus a thumbnail (when the app
 * declares `art`) and the two quick actions an operator reaches for most
 * (open + copy URL, publish/unpublish). Keeps the resting grid calm while
 * making every card a one-hover triage.
 */
export const AppHoverCard = ({
  app,
  showOrg = true,
  onPublish,
  onUnpublish,
  side = "right",
  children
}: AppHoverCardProps) => {
  const isLive = !!app.published_at;
  return (
    <HoverCard openDelay={160} closeDelay={80}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        side={side}
        align='start'
        className='w-72 overflow-hidden p-0'
        onClick={(e) => e.stopPropagation()}
      >
        {app.art_url && (
          // Thumbnail of the app's own art. object-cover crops to a clean
          // banner; the border-b seams it to the fact sheet below.
          <img
            src={app.art_url}
            alt=''
            className='aspect-video w-full border-b bg-muted object-cover'
          />
        )}
        <div className='space-y-3 p-3'>
          <div className='flex items-start gap-2'>
            <AppMark iconUrl={app.icon_url} name={app.name} size='md' />
            <div className='min-w-0 flex-1'>
              <div className='flex items-center gap-1.5'>
                <StatusDot isLive={isLive} />
                <span className='truncate font-medium text-xs'>{app.name}</span>
              </div>
              <span className='block truncate font-mono text-muted-foreground text-xs'>
                {app.org_slug}/{app.slug}
              </span>
            </div>
          </div>

          <dl className='grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-xs'>
            {showOrg && <Fact k='Org' v={app.org_slug} mono />}
            <Fact k='Project' v={app.project_id} mono />
            <Fact k='Branch' v={app.branch} mono />
            <Fact
              k='Last active'
              v={formatRelativeTime(app.last_active_at ?? app.last_synced_at)}
            />
            <Fact k='Last deploy' v={formatRelativeTime(app.last_deploy_at)} />
            {app.last_promoted_by_email && (
              <Fact
                k='Promoted'
                v={`${app.last_promoted_by_email}${
                  app.last_promoted_at ? ` · ${formatRelativeTime(app.last_promoted_at)}` : ""
                }`}
              />
            )}
          </dl>

          <div className='flex items-center gap-0.5 border-t pt-2'>
            <OpenAppButton url={app.url} />
            <CopyButton value={resolveBundleUrl(app.url)} label='app URL' />
            <Button
              variant='ghost'
              size='sm'
              className='ml-auto h-7 gap-1.5 px-2 text-xs'
              onClick={() => (isLive ? onUnpublish(app) : onPublish(app))}
            >
              {isLive ? <EyeOff className='size-3.5' /> : <Rocket className='size-3.5' />}
              {isLive ? "Unpublish" : "Publish"}
            </Button>
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
};

/** One term/definition pair in the fact sheet. `v` truncates with a full-value
 *  tooltip so a long project id or promoter email never blows out the width. */
const Fact = ({ k, v, mono }: { k: string; v: string; mono?: boolean }) => (
  <>
    <dt className='text-muted-foreground'>{k}</dt>
    <dd className={`min-w-0 truncate text-right ${mono ? "font-mono" : ""}`} title={v}>
      {v}
    </dd>
  </>
);
