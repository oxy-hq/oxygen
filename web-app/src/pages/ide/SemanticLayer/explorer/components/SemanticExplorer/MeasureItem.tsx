import { ArrowUpFromLine } from "lucide-react";
import { SidebarMenuSubButton, SidebarMenuSubItem } from "@/components/ui/shadcn/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";

interface MeasureItemProps {
  name: string;
  isSelected: boolean;
  onToggle: () => void;
  induced?: boolean;
  promotedFrom?: string;
}

const MeasureItem = ({ name, isSelected, onToggle, induced, promotedFrom }: MeasureItemProps) => (
  <SidebarMenuSubItem>
    <SidebarMenuSubButton
      onClick={onToggle}
      isActive={isSelected}
      data-testid={`semantic-field-measure-${name.replace(/[^A-Za-z0-9_]/g, "_")}`}
    >
      <span className='flex min-w-0 flex-1 items-center gap-1.5'>
        <span className='truncate'>{name}</span>
      </span>
      {induced && (
        <Tooltip>
          <TooltipTrigger asChild>
            <ArrowUpFromLine className='shrink-0 text-muted-foreground' size={12} />
          </TooltipTrigger>
          <TooltipContent side='right'>Induced from {promotedFrom}</TooltipContent>
        </Tooltip>
      )}
    </SidebarMenuSubButton>
  </SidebarMenuSubItem>
);

export default MeasureItem;
