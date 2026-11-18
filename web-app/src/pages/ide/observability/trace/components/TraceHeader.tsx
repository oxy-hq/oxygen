import { ArrowLeft, Clock, Layers } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { formatDuration, formatTimeAgo } from "../../utils/index";

interface TraceHeaderProps {
  traceId: string;
  totalDurationMs: number;
  spansCount: number;
  startTime: string;
}

export function TraceHeader({ traceId, totalDurationMs, spansCount, startTime }: TraceHeaderProps) {
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  return (
    <div className='flex items-center justify-between border-b px-2 py-1'>
      <div className='flex items-center gap-3'>
        <Button variant='ghost' size='icon' className='h-8 w-8' asChild>
          <Link to={ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACES}>
            <ArrowLeft className='h-4 w-4' />
          </Link>
        </Button>
        <div className='flex items-center gap-3'>
          <h1 className='font-semibold text-base'>Trace</h1>
          <code className='rounded bg-muted px-2 py-0.5 font-mono text-muted-foreground text-xs'>
            {traceId?.slice(0, 16)}...
          </code>
        </div>
      </div>
      <div className='flex items-center gap-4 text-muted-foreground text-xs'>
        <span className='flex items-center gap-1.5'>
          <Clock className='h-3.5 w-3.5' />
          {formatDuration(totalDurationMs)}
        </span>
        <span className='flex items-center gap-1.5'>
          <Layers className='h-3.5 w-3.5' />
          {spansCount} spans
        </span>
        <span>{formatTimeAgo(startTime)}</span>
      </div>
    </div>
  );
}
