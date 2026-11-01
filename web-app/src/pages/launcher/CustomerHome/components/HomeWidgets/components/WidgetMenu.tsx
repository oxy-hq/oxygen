import { EyeOff, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { cn } from "@/libs/shadcn/utils";

/**
 * A quiet control: out of sight until the pointer is over its card (`group/widget`
 * on the card) or focus is inside it, so a calm home stays calm. It is only
 * transparent, never removed: a keyboard reaches it, and on a touch screen, which
 * has no hover, it is always there.
 */
export const REVEAL_ON_HOVER =
  "opacity-0 group-focus-within/widget:opacity-100 group-hover/widget:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100 pointer-coarse:opacity-100";

/** A widget's "…" menu: for now only "Hide from my home". */
export function WidgetMenu({
  testId,
  label,
  onHide,
  className
}: {
  testId: string;
  /** What the menu is for, for a screen reader: the card's app and title. */
  label: string;
  onHide: () => void;
  className?: string;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant='ghost'
          size='icon'
          data-testid={testId}
          aria-label={`More: ${label}`}
          className={cn(
            "pointer-coarse:size-11 size-6 text-muted-foreground hover:text-foreground",
            REVEAL_ON_HOVER,
            className
          )}
        >
          <MoreHorizontal />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align='end'>
        <DropdownMenuItem data-testid='home-widget-hide' onSelect={onHide}>
          <EyeOff />
          Hide from my home
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
