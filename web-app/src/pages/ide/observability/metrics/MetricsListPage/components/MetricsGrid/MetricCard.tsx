import { ArrowUpRight } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { MetricAnalytics } from "@/services/api/metrics";
import { getRankColor } from "../../constants";

interface MetricCardProps {
  metric: MetricAnalytics;
  rank: number;
  maxCount: number;
  onClick: () => void;
}

export default function MetricCard({ metric, rank, maxCount, onClick }: MetricCardProps) {
  const percentage = (metric.count / maxCount) * 100;
  const isTop3 = rank <= 3;

  return (
    <div
      onClick={onClick}
      className={cn(
        "group relative cursor-pointer border p-4 transition-all duration-200",
        "hover:border-primary/50",
        isTop3 && "ring-1 ring-primary/20"
      )}
    >
      {/* Rank badge */}
      <div
        className={cn(
          "absolute -top-2 -left-2 flex h-7 w-7 items-center justify-center rounded-full font-bold text-xs",
          rank === 1 && "bg-rank-gold text-rank-gold-foreground",
          rank === 2 && "bg-rank-silver text-rank-silver-foreground",
          rank === 3 && "bg-rank-bronze text-rank-bronze-foreground",
          rank > 3 && "bg-muted text-muted-foreground"
        )}
      >
        {rank}
      </div>

      {/* Content */}
      <div className='space-y-3'>
        <div className='flex items-start justify-between gap-2'>
          <h3 className='line-clamp-2 font-semibold text-sm leading-tight transition-colors group-hover:text-primary'>
            {metric.name}
          </h3>
          <ArrowUpRight className='h-4 w-4 flex-shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100' />
        </div>

        {/* Usage bar */}
        <div className='space-y-1'>
          <div className='h-1.5 overflow-hidden rounded-full bg-muted'>
            <div
              className={cn("h-full rounded-full transition-all duration-500", getRankColor(rank))}
              style={{ width: `${percentage}%` }}
            />
          </div>
          <div className='flex items-center justify-between text-xs'>
            <span className='text-muted-foreground'>
              {metric.last_used ? new Date(metric.last_used).toLocaleDateString() : "—"}
            </span>
            <span className='font-medium text-foreground'>
              {metric.count.toLocaleString()} uses
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
