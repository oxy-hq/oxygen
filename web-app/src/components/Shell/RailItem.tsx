import { type ReactNode, useState } from "react";
import { useRailGlyph } from "./railGlyph";

/** One rail entry, in the rail (`ToggleRail`) and the phone's nav sheet
 *  (`ShellNavSheet`). Provide `imageUrl` (custom app mark, falls back to
 *  `letter` on load error), `icon`, or `letter`.
 *  `href` renders a real anchor (custom apps live outside the SPA);
 *  `onSelect` renders a button (SPA nav, wired by the caller).
 *  Router-free on purpose: the SDK will mount this inside custom apps. */
export type RailItem = {
  key: string;
  /** The entry's name: its accessible name, and the text the rail shows beside
   *  the mark once it opens. */
  label: string;
  testId: string;
  active?: boolean;
  icon?: ReactNode;
  letter?: string;
  imageUrl?: string;
  href?: string;
  onSelect?: () => void;
  /** Unread notifications, drawn as a dock badge on the mark. */
  badge?: number;
};

/** An item's mark: the app image (falling back to its letter on a load
 *  error), else its icon, else its letter. Shared with `ShellNavSheet`. */
export function RailItemIcon({ item }: { item: RailItem }) {
  const [imageFailed, setImageFailed] = useState(false);
  if (item.imageUrl && !imageFailed) {
    return (
      <img
        src={item.imageUrl}
        alt=''
        onError={() => setImageFailed(true)}
        className='h-5 w-5 rounded-sm object-contain'
      />
    );
  }
  return item.icon ?? <span className='font-semibold text-primary text-xs'>{item.letter}</span>;
}

/**
 * An app's mark as the reference draws it: no colour of its own, the row's text
 * colour (grey, the current one darker) through the mark cut out of its icon
 * (`railGlyph.ts`). Nothing while the cut is made, so the coloured icon never
 * flashes; the app's letter, in the same colour, when there is no mark to cut.
 */
function AppGlyph({ url, letter }: { url: string; letter?: string }) {
  const mask = useRailGlyph(url);
  if (mask === null) {
    return (
      <span data-glyph='letter' className='font-semibold text-xs'>
        {letter}
      </span>
    );
  }
  if (mask === undefined) return <span data-glyph='pending' className='size-5' />;
  return (
    <span
      data-glyph='mark'
      className='size-5 bg-current'
      style={{
        maskImage: `url("${mask}")`,
        WebkitMaskImage: `url("${mask}")`,
        // The mask carries the glyph's own margin (railGlyph.ts), so it fills the cell,
        // as the design draws a glyph (`size-5`).
        maskSize: "100%",
        WebkitMaskSize: "100%",
        maskRepeat: "no-repeat",
        WebkitMaskRepeat: "no-repeat",
        maskPosition: "center",
        WebkitMaskPosition: "center"
      }}
    />
  );
}

/** An entry's mark as the rail and the phone's nav sheet draw it: an app as its
 *  grey mark (`AppGlyph`), anything else as its icon or letter. */
export function RailMark({ item }: { item: RailItem }) {
  return item.imageUrl ? (
    <AppGlyph url={item.imageUrl} letter={item.letter} />
  ) : (
    <RailItemIcon item={item} />
  );
}
