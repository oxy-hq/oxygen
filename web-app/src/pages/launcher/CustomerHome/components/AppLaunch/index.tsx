import { createPortal } from "react-dom";
import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";

export { useAppLaunch } from "./useAppLaunch";

/**
 * The app's launch screen while it loads: its icon and name, centred on the
 * home's ground, over everything. Its geometry is the custom-app runtime's
 * (`runtime.js`, "launch screen": a 64px icon, 12px above a 14px name, the pair
 * centred), which draws the same screen as the app's first frame; change one and
 * change both, or the hand-over jumps.
 *
 * The progress bar waits before it shows: an app that opens quickly never shows
 * one, and one that does not says it is still coming.
 */
export function AppLaunchScreen({
  app,
  name,
  grown
}: {
  app: CustomAppSummary | null;
  /** The view-transition name the pressed tile hands over. */
  name: string;
  /** It grew from the tile: no fade of its own, which would show through the growth. */
  grown: boolean;
}) {
  if (!app) return null;
  return createPortal(
    <div
      role='status'
      data-testid='app-launch'
      data-app={app.slug}
      style={{ viewTransitionName: name }}
      className={cn(
        "fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-background",
        // A fade where the browser cannot grow it from the tile.
        !grown && "starting:opacity-0 transition-opacity duration-(--duration-fast)"
      )}
    >
      <div
        aria-hidden
        className='absolute inset-x-0 top-0 h-0.5 overflow-hidden starting:opacity-0 transition-opacity delay-700 duration-(--duration-slow)'
      >
        <div className='motion-launch-progress h-full w-2/5 bg-primary' />
      </div>
      <AppMark
        iconUrl={app.icon_url}
        name={app.name}
        size='xl'
        className='size-16 rounded-2xl text-xl'
      />
      <p className='max-w-full truncate px-6 font-medium text-foreground text-sm'>{app.name}</p>
      <span className='sr-only'>Opening {app.name}</span>
    </div>,
    document.body
  );
}
