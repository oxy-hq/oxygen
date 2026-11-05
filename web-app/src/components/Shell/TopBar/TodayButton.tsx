import { CalendarDays } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { TodayPanel } from "./TodayPanel";

/**
 * The minimal menu bar's way into Today: a calendar icon that opens the person's
 * widgets as a panel under the bar, from any page, the way a Mac shows its widgets
 * from the date. Today hidden on the home still lists them here, since the person
 * opens it on purpose.
 */
export function TodayButton() {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant='ghost'
          size='icon'
          data-testid='topbar-today-button'
          aria-label='Today'
          tooltip={open ? undefined : "Today"}
          className='size-7 text-muted-foreground hover:text-foreground'
        >
          <CalendarDays className='size-4' />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align='end'
        collisionPadding={0}
        data-testid='topbar-today'
        // Own scroll inside the room under the bar; a phone's panel is the screen's width.
        className='flex max-h-(--radix-popover-content-available-height) w-90 flex-col p-0 max-sm:w-screen max-sm:rounded-t-none max-sm:border-x-0'
      >
        <TodayPanel onNavigate={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}
