import { Search } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/shadcn/input-group";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { cn } from "@/libs/shadcn/utils";
import type { FrontlineStaff } from "@/types/frontline";
import { findByName, type RosterTile, recentRow, rosterFlow, showsNameSearch } from "./crewRoster";

/**
 * Columns follow the screen, not the roster: two on a phone, three on a
 * tablet held upright, four once a tablet is turned on its side. Two columns
 * on a 1280 px tablet left most of a 20-person roster scrolled away.
 */
const LIST_GRID = "grid grid-cols-2 gap-2 md:grid-cols-3 lg:landscape:grid-cols-4";
/** Always four across on a tablet, two by two on a phone. */
const RECENT_GRID = "grid grid-cols-2 gap-2 md:grid-cols-4";

/**
 * A name tile, big enough to hit with a thumb. The picked state is set LAST
 * and in full — fill, border, text and hover — so no other style on a tile
 * (the recent row's tint included) can leave a picked name pale.
 */
const TILE =
  "group relative h-auto min-h-14 w-full items-center justify-start whitespace-normal py-2 pr-2 pl-4 text-left font-medium text-base leading-tight data-[state=on]:border-primary data-[state=on]:bg-primary data-[state=on]:text-primary-foreground data-[state=on]:hover:bg-primary/90 data-[state=on]:hover:text-primary-foreground";

const SECTION_LABEL = "font-semibold text-muted-foreground text-xs uppercase tracking-wide";

/**
 * The shift board: every name on this kiosk's roster as a tile, in one A–Z
 * run that fills the screen below the title. Single-select — a worker picks
 * themself, then enters a PIN.
 *
 * The first name of each letter carries that letter in its corner and an A–Z
 * rail beside the list jumps to it, the way the tool these stores are moving
 * off does it: their home screen IS the roster and it carries an index. The
 * cap on the roster read is per store, so 200 names is reachable; past 30 a
 * "Find your name" box appears, and at a store big enough to hunt in, the
 * people who last signed in on this tablet sit at the top.
 */
interface CrewRosterPickerProps {
  staff: FrontlineStaff[];
  /** Identifiers of the people who last signed in on this kiosk, newest first. */
  recent?: string[];
  /** Where the tablet is, for the "Everyone at …" label. */
  placeName: string;
  /** Identifier of the picked crew member; empty when nobody is picked yet. */
  selected: string;
  onSelect: (identifier: string) => void;
  disabled?: boolean;
}

const CrewRosterPicker = ({
  staff,
  recent = [],
  placeName,
  selected,
  onSelect,
  disabled
}: CrewRosterPickerProps) => {
  const [query, setQuery] = useState("");
  const searching = query.trim().length > 0;
  const tiles = useMemo(() => rosterFlow(findByName(staff, query)), [staff, query]);
  const regulars = recentRow(staff, recent, { searching });
  const recentHeadingId = useId();
  const listRef = useRef<HTMLDivElement>(null);
  const letterTiles = useRef(new Map<string, HTMLElement>());

  // Radix clears the value when the pressed tile is tapped again; on a kiosk a
  // second tap on your own name must not un-pick you.
  const pick = (value: string) => {
    if (value) {
      onSelect(value);
    }
  };

  // A pick opens the PIN step, which on a phone or an upright tablet takes the
  // bottom of the screen and shortens this box in the same render — so the
  // tile just tapped can land under the new bottom edge. Bring it back into
  // view, moving the box only (never the page) and only as far as it takes.
  useEffect(() => {
    const list = listRef.current;
    const tile = [...(list?.querySelectorAll<HTMLElement>("[data-identifier]") ?? [])].find(
      (el) => el.dataset.identifier === selected
    );
    if (!list || !tile) {
      return;
    }
    const box = list.getBoundingClientRect();
    const at = tile.getBoundingClientRect();
    if (at.bottom > box.bottom) {
      list.scrollTo({ top: list.scrollTop + at.bottom - box.bottom, behavior: "smooth" });
    } else if (at.top < box.top) {
      list.scrollTo({ top: list.scrollTop + at.top - box.top, behavior: "smooth" });
    }
  }, [selected]);

  const jumpTo = (letter: string) => {
    const list = listRef.current;
    const tile = letterTiles.current.get(letter);
    if (!list || !tile) {
      return;
    }
    // Scroll the BOX, not the page. `scrollIntoView` walks every scrollable
    // ancestor, and on a tablet that drags the whole kiosk screen with it.
    const top =
      tile.getBoundingClientRect().top - list.getBoundingClientRect().top + list.scrollTop;
    list.scrollTo({ top, behavior: "smooth" });
    // And land ON the name, so a keyboard carries on from there with the
    // arrow keys instead of starting again from the top of the list.
    tile.focus({ preventScroll: true });
  };

  return (
    <div className='flex min-h-0 flex-1 flex-col gap-3'>
      {showsNameSearch(staff) && (
        <InputGroup className='h-12 shrink-0'>
          <InputGroupAddon>
            <Search aria-hidden='true' />
          </InputGroupAddon>
          <InputGroupInput
            type='search'
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder='Find your name'
            aria-label='Find your name'
            autoComplete='off'
            autoCorrect='off'
            spellCheck={false}
            className='text-base md:text-base'
            data-testid='login-crew-search'
          />
        </InputGroup>
      )}

      {regulars.length > 0 && (
        <section className='flex shrink-0 flex-col gap-2' aria-labelledby={recentHeadingId}>
          <h2 id={recentHeadingId} className={SECTION_LABEL}>
            On this tablet recently
          </h2>
          <ToggleGroup
            type='single'
            value={selected}
            onValueChange={pick}
            disabled={disabled}
            aria-labelledby={recentHeadingId}
            className={cn(RECENT_GRID, "items-stretch")}
          >
            {regulars.map((member) => (
              <ToggleGroupItem
                key={member.identifier}
                value={member.identifier}
                variant='outline'
                className={cn(TILE, "bg-muted/60")}
                data-identifier={member.identifier}
                data-testid={`login-crew-recent-${member.identifier}`}
              >
                {member.name}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </section>
      )}

      <h2 className={SECTION_LABEL}>
        Everyone at {placeName} · {staff.length}
      </h2>

      {/* At least a row of names, even under a phone's PIN sheet: below that
          the screen scrolls rather than squeezing the list to a sliver. */}
      <div className='flex min-h-16 flex-1 items-stretch gap-1'>
        <div
          ref={listRef}
          // Negative margin + padding so focus rings survive the scroll clip.
          // `relative` is what makes the rail's jump arithmetic simple.
          className='relative -m-1 min-h-0 flex-1 overflow-y-auto p-1'
          data-testid='login-crew-roster'
        >
          {tiles.length > 0 ? (
            <ToggleGroup
              type='single'
              value={selected}
              onValueChange={pick}
              disabled={disabled}
              aria-label='Your name'
              className={cn(LIST_GRID, "items-stretch")}
            >
              {tiles.map((tile) => (
                <NameTile
                  key={tile.member.identifier}
                  tile={tile}
                  registerLetter={(el) => {
                    if (!tile.letter) {
                      return;
                    }
                    if (el) {
                      letterTiles.current.set(tile.letter, el);
                    } else {
                      letterTiles.current.delete(tile.letter);
                    }
                  }}
                />
              ))}
            </ToggleGroup>
          ) : (
            <p className='px-1 py-4 text-muted-foreground'>No one here by that name.</p>
          )}
        </div>

        {/* One letter is not an index: with every name under the same initial
            there is nowhere to jump to, so the rail would be decoration on a
            screen that has no room for any. */}
        <LetterRail letters={railLetters(tiles)} onJump={jumpTo} disabled={disabled} />
      </div>
    </div>
  );
};

const railLetters = (tiles: RosterTile[]) =>
  tiles.flatMap((tile) => (tile.letter ? [tile.letter] : []));

/**
 * One name. The first of its letter wears the letter, small and grey, in the
 * corner — hidden from screen readers, which should hear "Ana", not "A Ana".
 */
const NameTile = ({
  tile: { member, letter },
  registerLetter
}: {
  tile: RosterTile;
  registerLetter: (el: HTMLButtonElement | null) => void;
}) => (
  <ToggleGroupItem
    ref={registerLetter}
    value={member.identifier}
    variant='outline'
    className={TILE}
    data-identifier={member.identifier}
    data-testid={`login-crew-staff-${member.identifier}`}
  >
    {letter && (
      <span
        aria-hidden='true'
        data-letter-mark=''
        className='absolute top-1 left-1.5 font-semibold text-muted-foreground text-xs leading-none group-data-[state=on]:text-primary-foreground/70'
      >
        {letter}
      </span>
    )}
    {member.name}
  </ToggleGroupItem>
);

/** What a screen reader says for a rail letter. `#` is not a letter anyone says. */
const railLabel = (letter: string) =>
  letter === "#" ? "Jump to names that start with a number or symbol" : `Jump to ${letter}`;

/**
 * The A–Z index down the side of the list.
 *
 * Only the letters the roster actually has: a rail of 26 with 21 dead is a row
 * of targets that answer nothing. The letters spread over the list's height,
 * so a short roster gets big targets; a full alphabet in a short box scrolls
 * rather than shrinking its letters below a thumb.
 */
const LetterRail = ({
  letters,
  onJump,
  disabled
}: {
  letters: string[];
  onJump: (letter: string) => void;
  disabled?: boolean;
}) =>
  letters.length > 1 ? (
    // A `nav`, because that is what it is: in-page navigation over the list
    // beside it. Every name is reachable by scrolling, so this is a shortcut,
    // never the only way to a tile.
    <nav
      // `justify-between` spreads the letters; once they overflow it behaves
      // as `start`, so the first letters never sit out of reach above the
      // scroll origin the way a centred overflowing column would.
      className='flex w-7 shrink-0 flex-col justify-between overflow-y-auto'
      aria-label='Jump to a letter'
      data-testid='login-crew-letter-rail'
    >
      {letters.map((letter) => (
        <button
          key={letter}
          type='button'
          // Not a form control and never a submit: this only moves the list.
          onClick={() => onJump(letter)}
          disabled={disabled}
          aria-label={railLabel(letter)}
          className='flex max-h-8 min-h-5 flex-1 items-center justify-center rounded-sm font-semibold text-muted-foreground text-xs leading-none hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50'
          data-testid={`login-crew-letter-${letter}`}
        >
          {letter}
        </button>
      ))}
    </nav>
  ) : null;

/** Same grid, no names yet — keeps the board from jumping when the roster lands. */
export const CrewRosterSkeleton = () => (
  <div className={LIST_GRID} aria-hidden='true'>
    {[0, 1, 2, 3, 4, 5, 6, 7].map((slot) => (
      <Skeleton key={slot} className='h-14' />
    ))}
  </div>
);

export default CrewRosterPicker;
