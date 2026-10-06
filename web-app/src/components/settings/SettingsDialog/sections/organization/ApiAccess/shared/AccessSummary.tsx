import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { AccessDescription } from "../utils/grants";

/**
 * Access as one phrase, with every grant spelled out behind it. The phrase
 * only becomes a tooltip trigger when the details say more than it does, so a
 * single grant doesn't advertise a hover that repeats it.
 */
export function AccessSummary({ access }: { access: AccessDescription }) {
  const hasMore = access.details.length > 1;
  if (!hasMore) return <span>{access.summary}</span>;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type='button'
          className='cursor-help rounded-sm text-left underline decoration-dotted underline-offset-2 focus-visible:outline-2 focus-visible:outline-ring'
        >
          {access.summary}
        </button>
      </TooltipTrigger>
      <TooltipContent side='bottom' className='max-w-xs'>
        <ul className='flex flex-col gap-0.5 text-left text-xs'>
          {access.details.map((line) => (
            <li key={line}>{line}</li>
          ))}
        </ul>
      </TooltipContent>
    </Tooltip>
  );
}
