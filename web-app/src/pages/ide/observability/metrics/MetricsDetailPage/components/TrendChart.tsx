import { TrendingUp } from "lucide-react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { cn } from "@/libs/shadcn/utils";
import type { MetricDetailResponse } from "@/services/api/metrics";

interface TrendChartProps {
  detailData: MetricDetailResponse;
  daysFilter: number;
}

export default function TrendChart({ detailData, daysFilter }: TrendChartProps) {
  const trends = detailData.usage_trend || [];

  if (!trends || trends.length === 0) {
    return (
      <Card className='bg-transparent shadow-none lg:col-span-2'>
        <CardHeader>
          <div className='flex items-center gap-2'>
            <TrendingUp className='h-5 w-5 text-primary' />
            <CardTitle>Usage Trend</CardTitle>
          </div>
          <CardDescription>Query frequency over the last {daysFilter} days</CardDescription>
        </CardHeader>
        <CardContent>
          <div className='flex h-48 items-center justify-center text-muted-foreground'>
            No trend data available
          </div>
        </CardContent>
      </Card>
    );
  }

  const maxUsage = Math.max(...trends.map((t) => t.count), 1);
  const avgUsage = Math.round(trends.reduce((sum, t) => sum + t.count, 0) / trends.length);

  return (
    <Card className='bg-transparent shadow-none lg:col-span-2'>
      <CardHeader>
        <div className='flex items-center gap-2'>
          <TrendingUp className='h-5 w-5 text-primary' />
          <CardTitle>Usage Trend</CardTitle>
        </div>
        <CardDescription>Query frequency over the last {daysFilter} days</CardDescription>
      </CardHeader>
      <CardContent>
        <div className='space-y-4'>
          {/* Stats row */}
          <div className='flex items-center gap-6 text-sm'>
            <div className='flex items-center gap-2'>
              <div className='h-3 w-3 rounded-full bg-primary' />
              <span className='text-muted-foreground'>Peak:</span>
              <span className='font-medium'>{maxUsage}</span>
            </div>
            <div className='flex items-center gap-2'>
              <div className='h-3 w-3 rounded-full bg-primary/50' />
              <span className='text-muted-foreground'>Avg:</span>
              <span className='font-medium'>{avgUsage}</span>
            </div>
          </div>

          {/* Chart */}
          <div className='relative h-48'>
            {/* Horizontal grid lines */}
            <div className='pointer-events-none absolute inset-0 flex flex-col justify-between'>
              {[...Array(5)].map((_, i) => (
                <div key={i} className='border-muted/30 border-t' />
              ))}
            </div>

            {/* Bars */}
            <div className='absolute inset-0 flex items-end gap-px'>
              {trends.map((point, index) => {
                const height = (point.count / maxUsage) * 100;
                const showLabel = index % Math.ceil(trends.length / 7) === 0;

                return (
                  <div key={index} className='flex h-full flex-1 flex-col items-center'>
                    <div className='flex w-full flex-1 items-end justify-center px-0.5'>
                      <div
                        className={cn(
                          "w-full cursor-pointer rounded-t transition-all duration-300",
                          "bg-gradient-to-t from-special to-special/70",
                          "hover:from-special hover:to-special/90",
                          "group relative"
                        )}
                        style={{
                          height: `${height}%`,
                          minHeight: point.count > 0 ? "4px" : "0"
                        }}
                      >
                        {/* Tooltip */}
                        <div className='absolute bottom-full left-1/2 z-20 mb-2 hidden -translate-x-1/2 group-hover:block'>
                          <div className='whitespace-nowrap border bg-popover px-3 py-2 text-popover-foreground text-xs shadow-lg'>
                            <p className='font-semibold'>{point.count} queries</p>
                            <p className='text-muted-foreground'>
                              {new Date(point.date).toLocaleDateString("en-US", {
                                weekday: "short",
                                month: "short",
                                day: "numeric"
                              })}
                            </p>
                          </div>
                        </div>
                      </div>
                    </div>
                    {showLabel && (
                      <div className='mt-2 whitespace-nowrap text-[10px] text-muted-foreground'>
                        {new Date(point.date).toLocaleDateString("en-US", {
                          month: "short",
                          day: "numeric"
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
