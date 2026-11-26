import { ArrowLeft, ArrowRight, RotateCw } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import type { PreviewHistory } from "./usePreviewHistory";

/**
 * Back / forward / reload for the **previewed app**, plus the location it is
 * showing.
 *
 * The browser's own controls cannot serve this. The frame's navigations are
 * kept out of the joint session history on purpose (see `previewHistory.ts`),
 * which is what makes the admin console's Back reliable — and the cost of that
 * is that the browser no longer knows the preview moved. This row is where the
 * capability goes instead, and putting it directly above the canvas is what
 * makes the two histories legible as two: the window chrome walks the admin
 * console, this walks the app inside it.
 *
 * The location is a read-out, not an input. An editable address bar invites
 * pointing the frame at an arbitrary origin, and the admin console should not
 * lend its same-origin frame to a URL somebody typed.
 */
export const PreviewNav = ({
  history,
  path
}: {
  history: PreviewHistory;
  /** The app-relative path, which is what the admin URL stores and what an
   *  operator would paste into a message. Falls back to the absolute URL
   *  before the first load resolves. */
  path: string | null;
}) => {
  // Nothing to steer: the frame is unreachable (cross-origin) or has not
  // loaded. Offering dead controls is worse than offering none.
  if (!history.available) return null;

  return (
    <div
      data-testid='admin-app-preview-nav'
      className='flex h-9 shrink-0 items-center gap-1 border-b bg-background px-2'
    >
      <NavButton
        label='Back in the previewed app'
        testId='admin-app-preview-nav-back'
        disabled={!history.canBack}
        onClick={history.back}
      >
        <ArrowLeft className='size-3.5' aria-hidden />
      </NavButton>
      <NavButton
        label='Forward in the previewed app'
        testId='admin-app-preview-nav-forward'
        disabled={!history.canForward}
        onClick={history.forward}
      >
        <ArrowRight className='size-3.5' aria-hidden />
      </NavButton>
      <NavButton
        label='Reload the previewed app'
        testId='admin-app-preview-nav-reload'
        disabled={!history.url}
        onClick={history.reload}
      >
        <RotateCw className='size-3.5' aria-hidden />
      </NavButton>

      {/* The app's own location, selectable so it can be copied — the admin
          URL above carries it too, but this is the one that reads as the
          app's. */}
      <span
        data-testid='admin-app-preview-nav-location'
        title={history.url ?? undefined}
        className='ml-1 min-w-0 flex-1 select-text truncate bg-muted/60 px-2 py-0.5 font-mono text-muted-foreground text-xs'
      >
        {path ?? history.url ?? "—"}
      </span>
    </div>
  );
};

const NavButton = ({
  label,
  testId,
  disabled,
  onClick,
  children
}: {
  label: string;
  testId: string;
  disabled: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) => (
  <Tooltip>
    <TooltipTrigger asChild>
      {/* Wrapped: a disabled button fires no pointer events, so the tooltip
          would never show on the state that most needs explaining. */}
      <span>
        <Button
          variant='ghost'
          size='icon'
          className='size-6 text-muted-foreground hover:text-foreground'
          aria-label={label}
          data-testid={testId}
          disabled={disabled}
          onClick={onClick}
        >
          {children}
        </Button>
      </span>
    </TooltipTrigger>
    <TooltipContent className={DESIGN_REFERENCE} side='bottom'>
      {label}
    </TooltipContent>
  </Tooltip>
);
