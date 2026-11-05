import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { HomeView } from "../useHomeView";

/** Simple or full: the home's one setting, at its top right, two words. */
export function HomeViewSwitch({
  view,
  onChange
}: {
  view: HomeView;
  onChange: (view: HomeView) => void;
}) {
  return (
    <ToggleGroup
      type='single'
      size='sm'
      variant='outline'
      value={view}
      // Radix answers "" when the pressed item is pressed again: keep the view.
      onValueChange={(next) => (next === "simple" || next === "full") && onChange(next)}
      aria-label='Home view'
      data-testid='home-view-switch'
      className='shrink-0'
    >
      <ToggleGroupItem value='simple' data-testid='home-view-simple' className='px-3 text-xs'>
        Simple
      </ToggleGroupItem>
      <ToggleGroupItem value='full' data-testid='home-view-full' className='px-3 text-xs'>
        Full
      </ToggleGroupItem>
    </ToggleGroup>
  );
}
