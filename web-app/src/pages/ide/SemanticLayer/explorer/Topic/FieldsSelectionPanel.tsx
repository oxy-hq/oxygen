import { ChevronDown, ChevronRight } from "lucide-react";
import { useMemo, useState } from "react";
import {
  SidebarContent,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub
} from "@/components/ui/shadcn/sidebar";

import CollapsibleFieldSection from "../components/SemanticExplorer/CollapsibleFieldSection";
import DimensionItem from "../components/SemanticExplorer/DimensionItem";
import MeasureItem from "../components/SemanticExplorer/MeasureItem";
import {
  isTimeDimension,
  useTimeDimensionHandlers
} from "../components/SemanticExplorer/useTimeDimensionHandlers";
import { useTopicExplorerContext } from "./contexts/TopicExplorerContext";

const FieldsSelectionPanel = () => {
  const {
    topicData,
    viewsWithData,
    topicLoading,
    selectedDimensions,
    selectedMeasures,
    toggleDimension,
    toggleMeasure,
    timeDimensions,
    onAddTimeDimension,
    onUpdateTimeDimension,
    onRemoveTimeDimension
  } = useTopicExplorerContext();

  const [expandedViews, setExpandedViews] = useState<Set<string> | null>(null);

  const { handleGranularitySelect, getSelectedGranularity } = useTimeDimensionHandlers(
    timeDimensions,
    onAddTimeDimension,
    onUpdateTimeDimension,
    onRemoveTimeDimension
  );

  const viewNames = useMemo(() => viewsWithData.map((v) => v.viewName), [viewsWithData]);

  const effectiveExpandedViews = useMemo(
    () => expandedViews ?? new Set(viewNames),
    [expandedViews, viewNames]
  );

  const toggleViewExpanded = (viewName: string) => {
    setExpandedViews((prev) => {
      const current = prev ?? new Set(viewNames);
      const next = new Set(current);
      if (next.has(viewName)) {
        next.delete(viewName);
      } else {
        next.add(viewName);
      }
      return next;
    });
  };

  if (!topicData) return null;

  return (
    <div className='flex w-72 flex-col overflow-hidden border-r bg-sidebar-background'>
      <SidebarGroupLabel className='flex h-auto min-h-12.5 items-center justify-between rounded-none border-sidebar-border border-b px-2 py-1'>
        <span className='font-semibold text-sm'>{topicData.name}</span>
      </SidebarGroupLabel>

      <SidebarContent className='h-full flex-1 overflow-y-auto'>
        <SidebarGroup className='px-1 pt-2'>
          {topicLoading && (
            <p className='px-3 py-2 text-muted-foreground text-xs'>Loading views...</p>
          )}

          {viewsWithData.length === 0 && !topicLoading && (
            <p className='px-3 py-2 text-muted-foreground text-xs'>
              No views found. Check that view files exist in the expected locations.
            </p>
          )}

          <SidebarMenu>
            {viewsWithData.map((view) => {
              const isExpanded = effectiveExpandedViews.has(view.viewName);
              const isBaseView = view.viewName === topicData.base_view;

              return (
                <SidebarMenuItem key={view.viewName}>
                  <SidebarMenuButton onClick={() => toggleViewExpanded(view.viewName)}>
                    {isExpanded ? (
                      <ChevronDown className='h-4 w-4' />
                    ) : (
                      <ChevronRight className='h-4 w-4' />
                    )}
                    <span className='flex items-center gap-1'>
                      {view.viewName}
                      {isBaseView && (
                        <span className='font-normal text-muted-foreground text-xs'>(base)</span>
                      )}
                    </span>
                  </SidebarMenuButton>

                  {isExpanded && (
                    <SidebarMenuSub className='ml-[15px]'>
                      {view.dimensions.length > 0 && (
                        <CollapsibleFieldSection title='Dimensions' count={view.dimensions.length}>
                          {view.dimensions.map((dimension) => {
                            const fullName = `${view.viewName}.${dimension.name}`;
                            const isTime = isTimeDimension(dimension.type);
                            const selectedGranularity = getSelectedGranularity(fullName);

                            return (
                              <DimensionItem
                                key={fullName}
                                name={dimension.name}
                                fullName={fullName}
                                type={dimension.type}
                                isSelected={
                                  selectedDimensions.includes(fullName) || !!selectedGranularity
                                }
                                selectedGranularity={selectedGranularity}
                                isTimeDimension={isTime}
                                onToggle={() => toggleDimension(fullName)}
                                onGranularitySelect={handleGranularitySelect}
                              />
                            );
                          })}
                        </CollapsibleFieldSection>
                      )}

                      {view.measures.length > 0 && (
                        <CollapsibleFieldSection title='Measures' count={view.measures.length}>
                          {view.measures.map((measure) => {
                            const fullName = `${view.viewName}.${measure.name}`;
                            return (
                              <MeasureItem
                                key={fullName}
                                name={measure.name}
                                isSelected={selectedMeasures.includes(fullName)}
                                onToggle={() => toggleMeasure(fullName)}
                                induced={measure.induced}
                                promotedFrom={measure.promoted_from}
                              />
                            );
                          })}
                        </CollapsibleFieldSection>
                      )}
                    </SidebarMenuSub>
                  )}
                </SidebarMenuItem>
              );
            })}
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
    </div>
  );
};

export default FieldsSelectionPanel;
