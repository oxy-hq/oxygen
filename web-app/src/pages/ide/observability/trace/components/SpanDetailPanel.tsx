import { Panel, PanelHeader } from "@/components/ui/panel";
import { Badge } from "@/components/ui/shadcn/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { cn } from "@/libs/shadcn/utils";
import type { TimelineSpan } from "@/services/api/traces";
import { formatDuration, formatSpanLabel, SpanIcon } from "../../utils/index";
import { InspectorAttributes } from "./InspectorAttributes";
import { InspectorEvents } from "./InspectorEvents";
import { InspectorOverview } from "./InspectorOverview";
import { InspectorSql } from "./InspectorSql";
import { getSpanCategory, isErrorStatus, SPAN_CATEGORY_META } from "./spanCategory";

interface SpanDetailPanelProps {
  span: TimelineSpan;
  /** Self time (ms) for this span, from the trace metrics. */
  selfMs: number;
  onClose: () => void;
}

export function SpanDetailPanel({ span, selfMs, onClose }: SpanDetailPanelProps) {
  const category = getSpanCategory(span);
  const { dotClass } = SPAN_CATEGORY_META[category];
  const isError = isErrorStatus(span.statusCode);

  return (
    <Panel>
      <PanelHeader
        title={
          <div className='flex items-center gap-2'>
            <span className={cn("h-2.5 w-2.5 shrink-0 ", dotClass)} />
            <SpanIcon spanName={span.spanName} className='h-4 w-4 shrink-0 text-muted-foreground' />
            <h2 className='truncate font-semibold text-sm' title={span.spanName}>
              {formatSpanLabel(span.spanName)}
            </h2>
          </div>
        }
        subtitle={
          <div className='mt-1.5 flex flex-wrap items-center gap-2'>
            <Badge variant={isError ? "destructive" : "secondary"} className='text-xs'>
              {span.statusCode || "Unset"}
            </Badge>
            <Badge variant='outline' className='font-mono text-xs'>
              {formatDuration(span.durationMs)}
            </Badge>
            <Badge variant='outline' className='text-xs'>
              {category}
            </Badge>
          </div>
        }
        onClose={onClose}
      />

      <Tabs defaultValue='overview' className='flex flex-1 flex-col overflow-hidden'>
        <div className='border-b px-2'>
          <TabsList className='flex h-auto items-start justify-start gap-1 rounded-none border-none bg-transparent p-0 py-2'>
            {["overview", "attributes", "sql", "events"].map((tab) => (
              <TabsTrigger
                key={tab}
                value={tab}
                className='px-3 py-1.5 text-xs capitalize transition-colors hover:text-foreground data-[state=active]:bg-accent! data-[state=active]:text-foreground data-[state=inactive]:text-muted-foreground'
              >
                {tab}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>

        <div className='scrollbar-gutter-auto flex-1 overflow-auto'>
          <TabsContent value='overview' className='mt-0 p-4'>
            <InspectorOverview span={span} selfMs={selfMs} />
          </TabsContent>
          <TabsContent value='attributes' className='mt-0 p-4'>
            <InspectorAttributes span={span} />
          </TabsContent>
          <TabsContent value='sql' className='mt-0 p-4'>
            {/* InspectorSql renders its own "no SQL" empty state. */}
            <InspectorSql span={span} />
          </TabsContent>
          <TabsContent value='events' className='mt-0 p-4'>
            <InspectorEvents span={span} />
          </TabsContent>
        </div>
      </Tabs>
    </Panel>
  );
}
