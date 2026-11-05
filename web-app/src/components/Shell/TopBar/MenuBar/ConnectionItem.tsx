import { WifiOff } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import useOnlineStatus from "@/hooks/useOnlineStatus";

/**
 * The menu bar's connection item: nothing while the browser is online, a red
 * "Offline" glyph when it is not. Being online is the normal state, and a signal
 * that only mirrors `navigator.onLine` says nothing about Oxygen's health, so it
 * earns room only when it has news.
 */
export function ConnectionItem() {
  const online = useOnlineStatus();
  if (online) return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role='img'
          aria-label='Offline'
          data-testid='menubar-connection'
          className='flex size-7 items-center justify-center text-destructive'
        >
          <WifiOff className='size-4' />
        </span>
      </TooltipTrigger>
      <TooltipContent>Offline: changes won't load until you reconnect</TooltipContent>
    </Tooltip>
  );
}
