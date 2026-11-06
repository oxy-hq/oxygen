import { useEffect, useRef } from "react";
import { cn } from "@/libs/shadcn/utils";
import useAskDock from "@/stores/useAskDock";
import { DockComposer } from "./DockComposer";
import { DockHeader } from "./DockHeader";
import { DockHistory } from "./DockHistory";
import { DockThread } from "./DockThread";
import { useDockWidth } from "./useDockWidth";

/**
 * The docked Ask panel — a right-side flex sibling of `<main>` that COMPACTS
 * the page when open (Cursor-style) rather than floating over it. It stays
 * mounted (collapsing to width 0) once first opened, so composer text and the
 * live thread survive a collapse/expand. The top-bar "Ask Oxygen" button and
 * ⌘K toggle it; the floating pill is gone.
 */
export function AskDock() {
  const isOpen = useAskDock((s) => s.isOpen);
  const view = useAskDock((s) => s.view);
  const threadId = useAskDock((s) => s.threadId);
  const { width, isDesktop, dragging, minWidth, maxWidth, handleProps } = useDockWidth();
  const asideRef = useRef<HTMLElement>(null);

  // The composer's `autoFocus` fires only when it mounts, and the dock stays
  // mounted across collapses — so every later ⌘K open left focus wherever it
  // was, and the first keystrokes went nowhere (#3970). Focus the active
  // view's textarea on each open instead; the history view has none.
  useEffect(() => {
    if (isOpen) asideRef.current?.querySelector("textarea")?.focus();
  }, [isOpen]);

  // Don't mount the dock (or its ChatPanel / Thread) until it's first opened —
  // but once mounted, keep it so collapsing never loses state.
  const everOpened = useRef(false);
  if (isOpen) everOpened.current = true;
  if (!everOpened.current) return null;

  return (
    <aside
      ref={asideRef}
      data-testid='ask-dock'
      // `inert` (not just aria-hidden) so the collapsed dock's textarea,
      // buttons, and links leave the tab order and the a11y tree together.
      inert={!isOpen}
      style={{ width: isOpen ? (isDesktop ? width : "100%") : 0 }}
      className={cn(
        "relative flex h-full shrink-0 flex-col overflow-hidden bg-background",
        isOpen ? "border-l" : "pointer-events-none",
        dragging ? "select-none" : "transition-[width] duration-150"
      )}
    >
      {isDesktop && isOpen && (
        // biome-ignore lint/a11y/useSemanticElements: focusable resize separator, not a static divider
        <div
          role='separator'
          aria-orientation='vertical'
          aria-label='Resize Ask panel'
          aria-valuenow={width}
          aria-valuemin={minWidth}
          aria-valuemax={maxWidth}
          tabIndex={0}
          data-testid='ask-dock-resize'
          {...handleProps}
          className='absolute inset-y-0 left-0 z-10 w-1.5 cursor-col-resize hover:bg-primary/20 focus-visible:bg-primary/30 focus-visible:outline-none'
        />
      )}
      <DockHeader />
      <div className='min-h-0 flex-1 overflow-auto'>
        {view === "composer" && <DockComposer />}
        {view === "thread" && threadId && <DockThread threadId={threadId} />}
        {view === "history" && <DockHistory />}
      </div>
    </aside>
  );
}
