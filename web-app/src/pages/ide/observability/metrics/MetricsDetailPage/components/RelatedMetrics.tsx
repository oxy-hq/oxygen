import { Hash, Sparkles } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { cn } from "@/libs/shadcn/utils";
import type { MetricDetailResponse } from "@/services/api/metrics";

interface RelatedMetricsProps {
  detailData: MetricDetailResponse;
  onMetricClick: (name: string) => void;
}

function RelatedMetricChip({ name, onClick }: { name: string; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "px-3 py-1.5 font-medium text-sm transition-all",
        "border border-transparent bg-muted/50 hover:border-primary/30 hover:bg-muted",
        "group flex min-w-0 items-center gap-1.5"
      )}
    >
      <Hash className='h-3 w-3 text-muted-foreground transition-colors group-hover:text-primary' />
      <span className='truncate'>{name}</span>
    </button>
  );
}

export default function RelatedMetrics({ detailData, onMetricClick }: RelatedMetricsProps) {
  const relatedMetrics = detailData.related_metrics;
  return (
    <Card className='bg-transparent shadow-none'>
      <CardHeader>
        <div className='flex items-center gap-2'>
          <Sparkles className='h-5 w-5 text-primary' />
          <CardTitle>Related Metrics</CardTitle>
        </div>
        <CardDescription>Often queried together</CardDescription>
      </CardHeader>
      <CardContent>
        {relatedMetrics.length > 0 ? (
          <div className='flex flex-wrap gap-2'>
            {relatedMetrics.map((related) => (
              <RelatedMetricChip
                key={related.name}
                name={related.name}
                onClick={() => onMetricClick(related.name)}
              />
            ))}
          </div>
        ) : (
          <p className='text-muted-foreground text-sm'>No related metrics found</p>
        )}
      </CardContent>
    </Card>
  );
}
