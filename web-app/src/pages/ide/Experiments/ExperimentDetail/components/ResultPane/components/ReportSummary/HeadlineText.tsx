import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";

/** Past this many characters the sentence runs past three lines on a report-width card. */
const LONG = 280;

/**
 * The server's headline sentence. A target filtered on hundreds of values names
 * every one in reports written before the server shortened the list, so a long
 * sentence shows three lines and opens on request.
 */
export default function HeadlineText({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const long = text.length > LONG;
  return (
    <div className='mt-1 text-muted-foreground text-sm'>
      <p
        className={cn("break-words", long && !open && "line-clamp-3")}
        data-testid='experiment-report-headline'
      >
        {text}
      </p>
      {long && (
        <Button
          variant='link'
          size='sm'
          className='h-auto p-0 text-xs'
          onClick={() => setOpen((o) => !o)}
        >
          {open ? "Show less" : "Show all"}
        </Button>
      )}
    </div>
  );
}
