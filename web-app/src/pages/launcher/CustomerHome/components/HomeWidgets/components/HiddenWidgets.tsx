import { Button } from "@/components/ui/shadcn/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import type { HiddenWidget } from "@/types/homeWidgets";

/**
 * Below Today, one quiet line, "2 hidden", that opens the list of what the person
 * hid, each with "Show". Nothing at all while nothing is hidden. A copy the person
 * could not open is never in the list (the server leaves it out).
 */
export function HiddenWidgets({
  hidden,
  onShow
}: {
  hidden: HiddenWidget[];
  onShow: (key: string) => void;
}) {
  if (hidden.length === 0) return null;
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type='button'
          data-testid='home-widgets-hidden'
          className='flex min-h-6 pointer-coarse:min-h-11 w-fit items-center text-muted-foreground text-sm transition-colors hover:text-foreground'
        >
          {hidden.length} hidden
        </button>
      </PopoverTrigger>
      <PopoverContent align='start' className='w-72 p-1' data-testid='home-widgets-hidden-list'>
        <ul className='flex flex-col'>
          {hidden.map((widget) => (
            <li
              key={widget.key}
              data-testid={`home-widget-hidden-${widget.key}`}
              className='flex items-center gap-2 px-2 py-1 text-sm'
            >
              <span className='flex min-w-0 flex-1 flex-col'>
                <span className='truncate text-foreground'>{widget.title}</span>
                <span className='truncate text-muted-foreground text-xs'>{widget.app.name}</span>
              </span>
              <Button
                variant='ghost'
                size='sm'
                data-testid='home-widget-show'
                aria-label={`Show ${widget.app.name}: ${widget.title}`}
                onClick={() => onShow(widget.key)}
              >
                Show
              </Button>
            </li>
          ))}
        </ul>
      </PopoverContent>
    </Popover>
  );
}
