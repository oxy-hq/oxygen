import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/shadcn/hover-card";
import {
  SidebarContent,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuSubButton,
  SidebarMenuSubItem
} from "@/components/ui/shadcn/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import usePreaggStatus from "@/hooks/api/usePreaggStatus";
import { formatDate, timeAgo } from "@/libs/utils/date";
import type { PreaggRollupStatus } from "@/services/api/semantic";
import {
  builtAt,
  CacheIcon,
  CacheState,
  FieldChips,
  MeasureChips
} from "../../components/preagg/RollupStatus";
import CollapsibleFieldSection from "../components/SemanticExplorer/CollapsibleFieldSection";
import DimensionItem from "../components/SemanticExplorer/DimensionItem";
import MeasureItem from "../components/SemanticExplorer/MeasureItem";
import {
  isTimeDimension,
  useTimeDimensionHandlers
} from "../components/SemanticExplorer/useTimeDimensionHandlers";
import { useViewExplorerContext } from "./contexts/ViewExplorerContext";

const RollupDetail = ({
  rollup,
  blobReads
}: {
  rollup: PreaggRollupStatus;
  blobReads: boolean;
}) => {
  const built = builtAt(rollup);
  return (
    <div className='space-y-3 text-xs'>
      <CacheState rollup={rollup} blobReads={blobReads} size='md' />

      {rollup.dimensions.length > 0 && (
        <div className='space-y-1'>
          <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-wide'>
            Dimensions
          </p>
          <FieldChips items={rollup.dimensions} />
        </div>
      )}

      {rollup.measures.length > 0 && (
        <div className='space-y-1'>
          <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-wide'>
            Measures
          </p>
          <MeasureChips measures={rollup.measures} />
        </div>
      )}

      {rollup.time_dimension && (
        <div className='space-y-1'>
          <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-wide'>
            Time dimension
          </p>
          <span className='font-mono text-foreground'>
            {rollup.time_dimension}
            {rollup.granularity && (
              <span className='ml-1 text-muted-foreground'>/ {rollup.granularity}</span>
            )}
          </span>
        </div>
      )}

      {built && (
        <div className='space-y-0.5'>
          <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-wide'>
            Built
          </p>
          <p className='text-foreground'>{formatDate(built)}</p>
          <p className='text-muted-foreground'>{timeAgo(built)}</p>
        </div>
      )}
    </div>
  );
};

const FieldsSelectionPanel = () => {
  const {
    viewData,
    selectedDimensions,
    setSelectedDimensions,
    selectedMeasures,
    setSelectedMeasures,
    toggleDimension,
    toggleMeasure,
    timeDimensions,
    onAddTimeDimension,
    onUpdateTimeDimension,
    onRemoveTimeDimension
  } = useViewExplorerContext();

  const { handleGranularitySelect, getSelectedGranularity } = useTimeDimensionHandlers(
    timeDimensions,
    onAddTimeDimension,
    onUpdateTimeDimension,
    onRemoveTimeDimension
  );

  // `error` is read, not just `data`: without it a failed status fetch made the
  // Pre-aggregations section vanish, which is indistinguishable from a view
  // that declares none. The tab reports the same failure out loud; the sidebar
  // should not disagree with it by staying quiet.
  const { data: preaggStatus, error: preaggError } = usePreaggStatus();

  if (!viewData) return null;

  const viewRollups = preaggStatus?.rollups.filter((r) => r.view_name === viewData.name) ?? [];
  // Same source the Pre-aggregation tab reads, so the two surfaces can't
  // disagree about whether a rollup built elsewhere still skips the warehouse.
  const blobReads = preaggStatus?.blob_reads_available ?? false;

  const dimensions = viewData.dimensions.map((dimension) => ({
    name: dimension.name,
    fullName: `${viewData.name}.${dimension.name}`,
    type: dimension.type
  }));

  const measures = viewData.measures.map((measure) => ({
    name: measure.name,
    fullName: `${viewData.name}.${measure.name}`,
    induced: measure.induced,
    promotedFrom: measure.promoted_from
  }));

  return (
    <div className='flex w-72 flex-col overflow-hidden border-r bg-sidebar-background'>
      <SidebarGroupLabel className='flex h-auto min-h-12.5 items-center justify-between rounded-none border-sidebar-border border-b px-2 py-1'>
        <span className='font-semibold text-sm'>{viewData.name}</span>
      </SidebarGroupLabel>

      {/* Metadata */}
      <div className='space-y-1.5 border-sidebar-border border-b px-3 py-2.5 text-sm'>
        <div className='flex items-center justify-between gap-2'>
          <span className='shrink-0 text-muted-foreground'>Data source</span>
          <span className='truncate text-foreground'>{viewData.datasource}</span>
        </div>
        <div className='flex items-center justify-between gap-2'>
          <span className='shrink-0 text-muted-foreground'>Table</span>
          <Tooltip>
            <TooltipTrigger asChild>
              <span className='max-w-[140px] cursor-help truncate text-foreground'>
                {viewData.table}
              </span>
            </TooltipTrigger>
            <TooltipContent side='left'>{viewData.table}</TooltipContent>
          </Tooltip>
        </div>
      </div>

      <SidebarContent className='h-full flex-1 overflow-y-auto'>
        <SidebarGroup className='px-1 pt-2'>
          <SidebarMenu>
            <CollapsibleFieldSection title='Dimensions' count={viewData.dimensions.length}>
              {dimensions.map((dimension) => {
                const isTime = isTimeDimension(dimension.type);
                const selectedGranularity = getSelectedGranularity(dimension.fullName);

                return (
                  <DimensionItem
                    key={dimension.name}
                    name={dimension.name}
                    fullName={dimension.fullName}
                    type={dimension.type}
                    isSelected={
                      selectedDimensions.includes(dimension.fullName) || !!selectedGranularity
                    }
                    selectedGranularity={selectedGranularity}
                    isTimeDimension={isTime}
                    onToggle={() => toggleDimension(dimension.fullName)}
                    onGranularitySelect={handleGranularitySelect}
                  />
                );
              })}
            </CollapsibleFieldSection>

            <CollapsibleFieldSection title='Measures' count={viewData.measures.length}>
              {measures.map((measure) => (
                <MeasureItem
                  key={measure.name}
                  name={measure.name}
                  isSelected={selectedMeasures.includes(measure.fullName)}
                  onToggle={() => toggleMeasure(measure.fullName)}
                  induced={measure.induced}
                  promotedFrom={measure.promotedFrom}
                />
              ))}
            </CollapsibleFieldSection>

            {(viewRollups.length > 0 || preaggError) && (
              <CollapsibleFieldSection
                title='Pre-aggregations'
                count={viewRollups.length}
                defaultOpen={false}
              >
                {preaggError && (
                  <p className='px-2 py-1 text-muted-foreground text-xs'>
                    Pre-aggregation status is unavailable right now. Queries still run — it is this
                    list, not the data, that is missing.
                  </p>
                )}
                {viewRollups.map((rollup: PreaggRollupStatus) => {
                  const applyRollup = () => {
                    setSelectedDimensions(rollup.dimensions.map((d) => `${viewData.name}.${d}`));
                    setSelectedMeasures(rollup.measures.map((m) => `${viewData.name}.${m.name}`));
                  };
                  return (
                    <SidebarMenuSubItem key={rollup.rollup_name}>
                      <HoverCard openDelay={300} closeDelay={100}>
                        <HoverCardTrigger asChild>
                          <SidebarMenuSubButton onClick={applyRollup}>
                            <CacheIcon rollup={rollup} blobReads={blobReads} />
                            <span className='truncate'>{rollup.rollup_name}</span>
                          </SidebarMenuSubButton>
                        </HoverCardTrigger>
                        <HoverCardContent side='right' align='start' className='w-64 p-3'>
                          <p className='mb-2 font-semibold text-sm'>{rollup.rollup_name}</p>
                          <RollupDetail rollup={rollup} blobReads={blobReads} />
                        </HoverCardContent>
                      </HoverCard>
                    </SidebarMenuSubItem>
                  );
                })}
              </CollapsibleFieldSection>
            )}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
    </div>
  );
};

export default FieldsSelectionPanel;
