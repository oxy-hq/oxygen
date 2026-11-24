import { ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { timeAgo } from "@/libs/utils/date";
import type { CustomApp } from "@/types/apps";
import type { LifecycleVerdict } from "../../appStatus";
import { AppIdentity } from "../AppIdentity";
import { LifecycleVerdictPill } from "../AppLifecycle";

/**
 * Which app this is, how it is, and the way out to the running thing.
 *
 * Its own file because it is a distinct named block of the console and because the
 * console around it is the densest component on this surface — the house rule is that a
 * JSX block with its own identity gets its own file rather than being scrolled past.
 *
 * The verdict comes in rather than being read here: the header's pill and the lifecycle
 * card's are one `useAppLifecycle` read in the console, so the pill beside the name can
 * never disagree with the card below it.
 */
export const ConsoleHeader = ({
  app,
  verdict,
  servedAt
}: {
  app: CustomApp;
  verdict: LifecycleVerdict;
  /** Where the app is actually served — its subdomain when it has one. */
  servedAt: string;
}) => (
  <header className='mb-4 flex flex-wrap items-start justify-between gap-3'>
    <div className='flex min-w-0 flex-col gap-1.5'>
      <AppIdentity app={app} size='lg' data-testid='apps-console-identity' />
      <div className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'>
        <LifecycleVerdictPill verdict={verdict} data-testid='apps-console-status' />
        <Since label='Published' at={app.published_at} />
        <Since label='Last active' at={app.last_active_at} />
      </div>
    </div>
    <Button asChild variant='outline' size='sm' className='h-7 gap-1.5 text-xs'>
      <a href={servedAt} target='_blank' rel='noreferrer' data-testid='apps-console-open'>
        Open app
        <ExternalLink className='size-3' />
      </a>
    </Button>
  </header>
);

const Since = ({ label, at }: { label: string; at: string | null | undefined }) => (
  <span>
    {label} {at ? <span title={new Date(at).toLocaleString()}>{timeAgo(at)}</span> : "—"}
  </span>
);
