import { cn } from "@/libs/shadcn/utils";
import type { WorldModel } from "@/types/worldModel";
import { worldModelCounts } from "./previewLayout";

const COUNT_LABELS = {
  entities: ["entity", "entities"],
  connections: ["connection", "connections"],
  measures: ["measure", "measures"]
} as const;

/**
 * The world model's three counts, figure first: "19 entities  19 connections  64
 * measures". On the home's card and the world model page, so the two say it the
 * same way.
 */
export function WorldModelCounts({ model, className }: { model: WorldModel; className?: string }) {
  const counts = worldModelCounts(model);
  return (
    <dl
      className={cn("flex flex-wrap gap-x-6 gap-y-2", className)}
      data-testid='world-model-counts'
    >
      {(Object.keys(COUNT_LABELS) as (keyof typeof COUNT_LABELS)[]).map((key) => {
        const [one, many] = COUNT_LABELS[key];
        return (
          <div key={key} className='flex items-baseline gap-1.5'>
            <dt className='text-muted-foreground text-sm'>{counts[key] === 1 ? one : many}</dt>
            {/* The figure reads first; the markup keeps term before value. */}
            <dd className='order-first font-medium text-base text-foreground tabular-nums'>
              {counts[key]}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}
