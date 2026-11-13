import { Calendar, ChevronDown, ChevronRight, Clock } from "lucide-react";
import { useState } from "react";
import {
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem
} from "@/components/ui/shadcn/sidebar";

const granularityOptions = [
  { value: "value", label: "value (raw)", icon: Clock },
  { value: "year", label: "year", icon: Calendar },
  { value: "quarter", label: "quarter", icon: Calendar },
  { value: "month", label: "month", icon: Calendar },
  { value: "week", label: "week", icon: Calendar },
  { value: "day", label: "day", icon: Calendar },
  { value: "hour", label: "hour", icon: Clock },
  { value: "minute", label: "minute", icon: Clock },
  { value: "second", label: "second", icon: Clock }
];

interface DimensionItemProps {
  name: string;
  fullName: string;
  type: string;
  isSelected: boolean;
  selectedGranularity?: string;
  isTimeDimension: boolean;
  onToggle: () => void;
  onGranularitySelect: (fullName: string, granularity: string) => void;
}

const DimensionItem = ({
  name,
  fullName,
  type,
  isSelected,
  selectedGranularity,
  isTimeDimension,
  onToggle,
  onGranularitySelect
}: DimensionItemProps) => {
  const [isExpanded, setIsExpanded] = useState(false);

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        onClick={isTimeDimension ? () => setIsExpanded(!isExpanded) : onToggle}
        isActive={isSelected}
        className='text-sidebar-foreground hover:bg-sidebar-accent'
      >
        {isTimeDimension && (isExpanded ? <ChevronDown /> : <ChevronRight />)}
        <span className='flex min-w-0 flex-1 items-center gap-1.5'>
          <span className='truncate'>{name}</span>
          {selectedGranularity && (
            <span className='shrink-0 text-muted-foreground text-xs'>({selectedGranularity})</span>
          )}
        </span>
        {isTimeDimension && <Clock />}
      </SidebarMenuButton>

      {isTimeDimension && isExpanded && (
        <SidebarMenuSub className='ml-[15px] box-border'>
          {granularityOptions
            .filter((opt) =>
              type === "date" ? !["hour", "minute", "second"].includes(opt.value) : true
            )
            .map((opt) => {
              const Icon = opt.icon;
              const active = selectedGranularity === opt.value;
              return (
                <SidebarMenuSubItem key={opt.value}>
                  <SidebarMenuSubButton
                    onClick={() => onGranularitySelect(fullName, opt.value)}
                    isActive={active}
                  >
                    <Icon />
                    <span>{opt.label}</span>
                  </SidebarMenuSubButton>
                </SidebarMenuSubItem>
              );
            })}
        </SidebarMenuSub>
      )}
    </SidebarMenuItem>
  );
};

export default DimensionItem;
