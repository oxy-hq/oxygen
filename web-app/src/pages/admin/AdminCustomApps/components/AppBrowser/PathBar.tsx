import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";

export interface PathSegment {
  label: string;
  /** Absent on the last segment: where you already are is not a link. */
  to?: string;
}

/**
 * Where you are, along the bottom — Finder's path bar.
 *
 * It earns its line twice. It is the way back up a tier without reaching for a column,
 * and it is what makes the collapse toggle safe: with the Organizations and Apps columns
 * hidden, this is the only thing still saying which org's app is on screen. Removing the
 * header's "All apps" button and its second copy of the app's name was possible because
 * this says both, once, in the place a file browser says it.
 */
export const PathBar = ({ segments }: { segments: readonly PathSegment[] }) => (
  <nav
    aria-label='Selection path'
    data-testid='admin-apps-browser-path'
    // The footer row owns the top border; a second one here drew a double rule beside
    // the collapse button.
    className='flex h-7 min-w-0 flex-1 items-center gap-1 overflow-x-auto px-3 text-muted-foreground text-xs'
  >
    {segments.map((segment, i) => (
      <span key={segment.label} className='flex shrink-0 items-center gap-1'>
        {i > 0 && <ChevronRight className='size-3 shrink-0 opacity-50' aria-hidden />}
        {segment.to ? (
          <Link to={segment.to} className='hover:text-foreground hover:underline'>
            {segment.label}
          </Link>
        ) : (
          <span className='text-foreground'>{segment.label}</span>
        )}
      </span>
    ))}
  </nav>
);
