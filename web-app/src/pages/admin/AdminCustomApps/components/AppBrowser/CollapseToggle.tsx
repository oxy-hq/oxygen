import { PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";

/**
 * Hide the Organizations and Apps tiers, leaving Pages and the pane.
 *
 * The console's Data and Functions pages want width, and three tiers take about 550px of
 * it before the working area starts. This is what pays that back — and it is safe to use
 * because the path bar beside it still says which org's app is on screen, so a collapsed
 * rail is not a lost one.
 *
 * It only appears with an app selected: with none, the pane holds the fleet table and
 * collapsing would leave the surface no navigation at all.
 */
export const CollapseToggle = ({
  collapsed,
  onToggle
}: {
  collapsed: boolean;
  onToggle: () => void;
}) => (
  <Button
    variant='ghost'
    size='sm'
    onClick={onToggle}
    className='h-7 shrink-0 gap-1.5 rounded-none border-r text-muted-foreground text-xs'
    data-testid='admin-apps-browser-collapse'
    aria-expanded={!collapsed}
  >
    {collapsed ? (
      <PanelLeftOpen className='size-3' aria-hidden />
    ) : (
      <PanelLeftClose className='size-3' aria-hidden />
    )}
    {collapsed ? "Show orgs" : "Hide orgs"}
  </Button>
);
