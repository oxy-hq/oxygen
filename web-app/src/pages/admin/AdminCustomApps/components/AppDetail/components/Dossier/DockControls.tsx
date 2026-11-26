import { PanelBottom, PanelRight, SquareArrowOutUpRight } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { DockMode } from "../../dock";

const OPTIONS: { value: DockMode; icon: typeof PanelRight; label: string }[] = [
  { value: "right", icon: PanelRight, label: "Dock to right" },
  { value: "bottom", icon: PanelBottom, label: "Dock to bottom" },
  { value: "window", icon: SquareArrowOutUpRight, label: "Open in a separate window" }
];

/**
 * The dock switcher: three icons, DevTools' own vocabulary and order. Icon-only
 * because it sits in a 36px strip that also has to hold a title — the tooltip
 * carries the wording, and each item keeps a full `aria-label`.
 */
export const DockControls = ({
  value,
  onChange
}: {
  value: DockMode;
  onChange: (next: DockMode) => void;
}) => (
  <ToggleGroup
    type='single'
    value={value}
    onValueChange={(next) => next && onChange(next as DockMode)}
    size='sm'
    variant='outline'
    aria-label='Details placement'
  >
    {OPTIONS.map(({ value: option, icon: Icon, label }) => (
      <Tooltip key={option}>
        <TooltipTrigger asChild>
          <ToggleGroupItem value={option} aria-label={label} className='size-6 px-0'>
            <Icon className='size-3.5' />
          </ToggleGroupItem>
        </TooltipTrigger>
        <TooltipContent className={DESIGN_REFERENCE}>{label}</TooltipContent>
      </Tooltip>
    ))}
  </ToggleGroup>
);
