import { useMemo } from "react";
import { Combobox } from "@/components/ui/shadcn/combobox";
import type { PersonOption } from "@/hooks/api/organizations/useOperatingGraph";
import { PERSON_KIND_MARK } from "@/libs/operatingGraph";

/** Radix/cmdk items can't carry an empty value, so "nobody" needs a name. Ids are uuids. */
export const NO_PERSON = "__none__";

/**
 * A searchable picker over everyone who can hold a position — members and
 * crew in one list, crew marked so a manager sees at a glance which rows
 * came from the kiosk roster.
 */
export function PersonSelect({
  people,
  value,
  onValueChange,
  placeholder = "Pick a person",
  allowNone = false,
  noneLabel = "Nobody",
  exclude = [],
  disabled,
  testId
}: {
  people: PersonOption[];
  value: string;
  onValueChange: (value: string) => void;
  placeholder?: string;
  /** Offer `NO_PERSON` as the first choice — for an optional supervisor. */
  allowNone?: boolean;
  noneLabel?: string;
  /** Ids to leave out, e.g. the person being supervised. */
  exclude?: string[];
  disabled?: boolean;
  testId: string;
}) {
  const byId = useMemo(() => new Map(people.map((p) => [p.id, p])), [people]);
  const items = useMemo(() => {
    const rows = people
      .filter((p) => !exclude.includes(p.id))
      .map((p) => ({
        value: p.id,
        label: PERSON_KIND_MARK[p.kind] ? `${p.name} (${PERSON_KIND_MARK[p.kind]})` : p.name,
        // The id keeps two people with the same name apart in cmdk's matcher.
        searchText: `${p.name} ${p.detail} ${p.id}`
      }));
    return allowNone
      ? [{ value: NO_PERSON, label: noneLabel, searchText: `${noneLabel} ${NO_PERSON}` }, ...rows]
      : rows;
  }, [people, exclude, allowNone, noneLabel]);

  return (
    // `min-w-0`: in a grid or flex row this wrapper must be allowed to shrink
    // below the combobox's content width, or the row overflows its dialog.
    <div data-testid={testId} className='min-w-0'>
      <Combobox
        items={items}
        value={value}
        onValueChange={onValueChange}
        placeholder={placeholder}
        searchPlaceholder='Search people...'
        disabled={disabled}
        renderItem={(item) => {
          const person = byId.get(item.value);
          if (!person) return <span className='text-muted-foreground'>{item.label}</span>;
          const mark = PERSON_KIND_MARK[person.kind];
          return (
            <span className='flex min-w-0 flex-1 items-baseline gap-2'>
              <span className='truncate'>{person.name}</span>
              {mark && (
                <span className='shrink-0 rounded-sm bg-muted px-1 text-muted-foreground text-xs'>
                  {mark}
                </span>
              )}
              <span className='ml-auto shrink-0 text-muted-foreground text-xs'>
                {person.detail}
              </span>
            </span>
          );
        }}
      />
    </div>
  );
}
