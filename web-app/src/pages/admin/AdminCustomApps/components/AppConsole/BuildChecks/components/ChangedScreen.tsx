import { ChevronRight } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { ChecksScope } from "../checksScope";
import { type ChangedReading, ratioText } from "../screensModel";
import { CheckPicture } from "./CheckPicture";

/** What production showed when its picture was taken, beside its caption. */
function productionCaption(screen: ChangedReading): string {
  if (screen.production_ready === false) return "Production · not ready";
  const errors = screen.production_errors.length;
  return errors ? `Production · ${errors} error${errors === 1 ? "" : "s"}` : "Production";
}

/**
 * A screen's two pictures, production then staging, with the diff picture where
 * staging's is while "Diff" is on. One scroll for the pair, so the two stay side by
 * side down a long page. A phone's pictures are held narrow: they are tall, and blown
 * up they show less.
 */
const ScreenPictures = ({
  scope,
  attempt,
  screen,
  diff
}: {
  scope: ChecksScope;
  attempt: number;
  screen: ChangedReading;
  diff: boolean;
}) => {
  const where = `${screen.name} ${screen.viewport}`.trim();
  return (
    <div
      className={cn(
        "mb-2 grid max-h-96 grid-cols-2 gap-2 overflow-y-auto",
        screen.viewport === "phone" && "max-w-md"
      )}
    >
      <CheckPicture
        scope={scope}
        attempt={attempt}
        name={screen.production}
        label={productionCaption(screen)}
        alt={`Production, ${where}`}
      />
      <CheckPicture
        key={diff ? "diff" : "staging"}
        scope={scope}
        attempt={attempt}
        name={diff ? screen.diff : screen.staging}
        label={diff ? "Diff" : "Staging"}
        alt={`${diff ? "Diff" : "Staging"}, ${where}`}
      />
    </div>
  );
};

/**
 * One screen at one viewport: its path, how much of it differs, and the two pictures
 * side by side, production then staging, scrolling together. A screen that changed
 * starts open; one that did not opens on a click. "Diff" puts the diff picture where
 * staging's is.
 */
export const ChangedScreen = ({
  scope,
  attempt,
  screen
}: {
  scope: ChecksScope;
  attempt: number;
  screen: ChangedReading;
}) => {
  const [open, setOpen] = useState(screen.changed);
  const [diff, setDiff] = useState(false);
  const share = ratioText(screen);
  return (
    <li
      className='border-border/40 border-b last:border-b-0'
      data-testid='build-check-change'
      data-screen={screen.key}
      data-changed={screen.changed}
    >
      <div className='flex items-center gap-2 text-xs'>
        <button
          type='button'
          aria-expanded={open}
          onClick={() => setOpen((was) => !was)}
          className='flex min-w-0 flex-1 items-center gap-2 py-1 text-left hover:bg-muted/40'
          data-testid='build-check-change-toggle'
        >
          <ChevronRight
            className={cn(
              "size-3 shrink-0 text-muted-foreground transition-transform",
              open && "rotate-90"
            )}
            aria-hidden
          />
          <span className='min-w-0 truncate font-mono' title={screen.name}>
            {screen.name}
          </span>
          <span className='shrink-0 text-muted-foreground'>{screen.viewport}</span>
        </button>
        {open && screen.diff && (
          <Button
            size='sm'
            variant={diff ? "secondary" : "ghost"}
            className='h-6 px-2 text-xs'
            aria-pressed={diff}
            onClick={() => setDiff((was) => !was)}
            data-testid='build-check-change-diff'
          >
            Diff
          </Button>
        )}
        <span
          className={cn(
            "shrink-0 tabular-nums",
            share ? cn("font-medium", ADMIN_TONE.warn.text) : "text-muted-foreground"
          )}
          data-testid='build-check-change-share'
        >
          {share || "same"}
        </span>
      </div>
      {open && <ScreenPictures scope={scope} attempt={attempt} screen={screen} diff={diff} />}
    </li>
  );
};
