import { Wifi, WifiOff } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import useOnlineStatus from "@/hooks/useOnlineStatus";
import { cn } from "@/libs/shadcn/utils";

/** The menu bar's connection item: the "Sys: Connected" signal as one icon. */
export function ConnectionItem() {
  const online = useOnlineStatus();
  const label = online ? "Connected" : "Offline";
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role='img'
          aria-label={label}
          data-testid='menubar-connection'
          data-online={online}
          className={cn(
            "hidden size-7 items-center justify-center sm:flex",
            online ? "text-muted-foreground" : "text-destructive"
          )}
        >
          {online ? <Wifi className='size-4' /> : <WifiOff className='size-4' />}
        </span>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
