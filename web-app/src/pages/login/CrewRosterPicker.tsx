import { useMemo, useRef } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { FrontlineStaff } from "@/types/frontline";
import { groupByInitial, type RosterGroup } from "./crewRoster";

const GRID_CLASS = "grid grid-cols-2 gap-2";

/**
 * The shift board: every name on this kiosk's roster as a tile big enough to
 * hit with a thumb. Single-select — a worker picks themself, then enters a PIN.
 *
 * Grouped by first letter with an A–Z rail beside it, the way the tool these
 * stores are moving off does it: their home screen IS the roster and it carries
 * an index. A flat grid in server order is a scroll at 26 names and a hunt at
 * 200, and the cap on the roster read is per store now, so 200 is reachable.
 */
interface CrewRosterPickerProps {
  staff: FrontlineStaff[];
  /** Identifier of the picked crew member; empty when nobody is picked yet. */
  selected: string;
  onSelect: (identifier: string) => void;
  disabled?: boolean;
}

const CrewRosterPicker = ({ staff, selected, onSelect, disabled }: CrewRosterPickerProps) => {
  const groups = useMemo(() => groupByInitial(staff), [staff]);
  const listRef = useRef<HTMLDivElement>(null);
  const groupEls = useRef(new Map<string, HTMLElement>());

  const jumpTo = (letter: string) => {
    const list = listRef.current;
    const group = groupEls.current.get(letter);
    if (!list || !group) {
      return;
    }
    // Scroll the BOX, not the page. `scrollIntoView` walks every scrollable
    // ancestor, so on a tablet it drags the login card itself up the screen
    // and the PIN field goes with it.
    //
    // Measure the GROUP, not its heading. The heading is sticky, so from
    // partway down a letter it is pinned to the top of the box and measures as
    // "already there" — tapping that letter to get back to its start did
    // nothing.
    const top =
      group.getBoundingClientRect().top - list.getBoundingClientRect().top + list.scrollTop;
    list.scrollTo({ top, behavior: "smooth" });
  };

  return (
    <div className='flex items-stretch gap-1'>
      <ToggleGroup
        ref={listRef}
        type='single'
        value={selected}
        // Radix clears the value when the pressed tile is tapped again; on a
        // kiosk a second tap on your own name must not un-pick you.
        onValueChange={(value) => {
          if (value) {
            onSelect(value);
          }
        }}
        disabled={disabled}
        aria-label='Your name'
        // Negative margin + padding so focus rings survive the scroll clip.
        // `relative` is what makes the rail's jump arithmetic simple.
        className='relative -m-1 max-h-72 flex-1 flex-col items-stretch justify-start gap-3 overflow-y-auto p-1'
        data-testid='login-crew-roster'
      >
        {groups.map((group) => (
          <LetterGroup
            key={group.letter}
            group={group}
            registerGroup={(el) => {
              if (el) {
                groupEls.current.set(group.letter, el);
              } else {
                groupEls.current.delete(group.letter);
              }
            }}
          />
        ))}
      </ToggleGroup>

      {/* One letter is not an index: with every name under the same initial
          there is nowhere to jump to, so the rail would be decoration on a
          screen that has no room for any. */}
      {groups.length > 1 && (
        <LetterRail letters={groups.map((g) => g.letter)} onJump={jumpTo} disabled={disabled} />
      )}
    </div>
  );
};

/** One letter's names, under a heading; the group is what the rail lands on. */
const LetterGroup = ({
  group,
  registerGroup
}: {
  group: RosterGroup;
  registerGroup: (el: HTMLElement | null) => void;
}) => (
  <div ref={registerGroup} className='flex flex-col gap-2'>
    {/* Sticky so the letter you jumped to stays readable while you scan under
        it. Opaque, or the tiles scroll through it. */}
    <h3 className='sticky top-0 z-10 bg-background py-0.5 font-medium text-muted-foreground text-xs'>
      {group.letter}
    </h3>
    <div className={GRID_CLASS}>
      {group.staff.map((member) => (
        <ToggleGroupItem
          key={member.identifier}
          value={member.identifier}
          variant='outline'
          className='h-auto min-h-12 px-2 py-2 text-center leading-tight data-[state=on]:border-primary data-[state=on]:bg-primary data-[state=on]:text-primary-foreground data-[state=on]:hover:bg-primary/90 data-[state=on]:hover:text-primary-foreground'
          data-testid={`login-crew-staff-${member.identifier}`}
        >
          {member.name}
        </ToggleGroupItem>
      ))}
    </div>
  </div>
);

/**
 * The A–Z index down the side.
 *
 * Only the letters the roster actually has: a rail of 26 with 21 dead is a row
 * of targets that answer nothing, and at three names it would be taller than
 * the list it indexes. A full alphabet at a 200-name store is 26 entries in a
 * 288 px box, so the rail scrolls in that one case rather than shrinking its
 * letters below a thumb.
 */
const LetterRail = ({
  letters,
  onJump,
  disabled
}: {
  letters: string[];
  onJump: (letter: string) => void;
  disabled?: boolean;
}) => (
  // A `nav`, because that is what it is: in-page navigation over the list
  // beside it. Every name is reachable by scrolling, so this is a shortcut,
  // never the only way to a tile.
  <nav
    // `justify-start`, not centred: a centred flex column that overflows puts
    // its first items out of reach above the scroll origin.
    className='flex max-h-72 flex-col justify-start gap-0.5 overflow-y-auto'
    aria-label='Jump to a letter'
    data-testid='login-crew-letter-rail'
  >
    {letters.map((letter) => (
      <button
        key={letter}
        type='button'
        // Not a form control and never a submit: this only moves the scroll box.
        onClick={() => onJump(letter)}
        disabled={disabled}
        className='flex min-h-5 min-w-5 items-center justify-center rounded-sm px-1 font-medium text-muted-foreground text-xs leading-none hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50'
        data-testid={`login-crew-letter-${letter}`}
      >
        {letter}
      </button>
    ))}
  </nav>
);

/** Same grid, no names yet — keeps the board from jumping when the roster lands. */
export const CrewRosterSkeleton = () => (
  <div className={GRID_CLASS} aria-hidden='true'>
    {[0, 1, 2, 3].map((slot) => (
      <Skeleton key={slot} className='h-12' />
    ))}
  </div>
);

export default CrewRosterPicker;
