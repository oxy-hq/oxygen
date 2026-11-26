import { cn } from "@/libs/shadcn/utils";
import type { MetricAnalytics } from "@/services/api/metrics";

interface MetricListItemProps {
  metric: MetricAnalytics;
  rank: number;
  maxCount: number;
  onClick: () => void;
}

export default function MetricListItem({ metric, rank, maxCount, onClick }: MetricListItemProps) {
  const percentage = (metric.count / maxCount) * 100;

  return (
    <div
      onClick={onClick}
      className={cn(
        "flex cursor-pointer items-center gap-4 border p-3 transition-all",
        "hover:border-primary/50"
      )}
    >
      <div
        className={cn(
          "flex h-8 w-8 items-center justify-center rounded-full font-bold text-sm",
          rank === 1 && "bg-rank-gold text-rank-gold-foreground",
          rank === 2 && "bg-rank-silver text-rank-silver-foreground",
          rank === 3 && "bg-rank-bronze text-rank-bronze-foreground",
          rank > 3 && "bg-muted text-muted-foreground"
        )}
      >
        {rank}
      </div>
      <div className='min-w-0 flex-1'>
        <p className='truncate font-medium text-sm'>{metric.name}</p>
        <p className='text-muted-foreground text-xs'>
          Last: {metric.last_used ? new Date(metric.last_used).toLocaleDateString() : "—"}
        </p>
      </div>
      <div className='h-2 w-24 overflow-hidden rounded-full bg-muted'>
        <div className='h-full rounded-full bg-primary' style={{ width: `${percentage}%` }} />
      </div>
      <div className='w-20 text-right'>
        <p className='font-semibold text-sm'>{metric.count.toLocaleString()}</p>
        <p className='text-muted-foreground text-xs'>queries</p>
      </div>
    </div>
  );
}
