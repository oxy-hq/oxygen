import { Search } from "lucide-react";
import type React from "react";
import { useEffect, useId, useRef, useState } from "react";
import AppSlug from "@/components/ui/AppSlug";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { groupSandboxApps, sandboxAppRef } from "@/libs/sandboxAgentToken";
import { cn } from "@/libs/shadcn/utils";
import { isSubmitChord } from "@/libs/submitChord";
import type { SandboxApp } from "@/types/apiToken";

interface Props {
  /** Every app the caller may mint for. Never empty: the type isn't offered without one. */
  apps: SandboxApp[];
  pickedIds: string[];
  /** The limit is reached: what isn't picked can't be, until something else is unticked. */
  full: boolean;
  onToggle: (id: string, on: boolean) => void;
  /** Names the keys the search field takes, for a screen reader. */
  keysHintId?: string;
}

interface RowProps {
  app: SandboxApp;
  checked: boolean;
  disabled: boolean;
  /** The row the search field's Enter would pick. */
  active: boolean;
  onToggle: (on: boolean) => void;
}

/** One app: a checkbox, its name, and the `<org>/<app>` reference oxyc knows it by. */
const AppRow: React.FC<RowProps> = ({ app, checked, disabled, active, onToggle }) => {
  const id = useId();
  const row = useRef<HTMLLIElement>(null);
  // Keeps the highlighted row in view as the arrow keys walk a list longer than its box.
  useEffect(() => {
    if (active) row.current?.scrollIntoView?.({ block: "nearest" });
  }, [active]);

  return (
    <li
      ref={row}
      className={cn(
        "flex h-9 items-center gap-3 rounded-md px-3 hover:bg-accent/60",
        active && "bg-accent hover:bg-accent"
      )}
      data-testid='account-token-sandbox-app'
      data-app-name={app.name}
      data-active={active ? "true" : undefined}
    >
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(on) => onToggle(on === true)}
        data-testid='account-token-sandbox-app-checkbox'
      />
      <label
        htmlFor={id}
        className={cn("min-w-0 flex-1 cursor-pointer truncate", checked && "font-medium")}
      >
        {app.name}
      </label>
      <AppSlug
        slug={sandboxAppRef(app)}
        tone={checked ? "strong" : "quiet"}
        className='min-w-0 shrink truncate'
      />
    </li>
  );
};

/**
 * The apps behind "Sandbox agent": searchable, under the org each belongs to. Staff can reach a
 * great many apps, and a token names a handful, so the search comes first.
 *
 * From the search field the arrow keys move a highlight down the matches and Enter picks the
 * highlighted one, so a token's apps can be named without leaving the keyboard. Enter never
 * submits the form from here.
 */
const SandboxAppPicker: React.FC<Props> = ({ apps, pickedIds, full, onToggle, keysHintId }) => {
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [cursor, setCursor] = useState(0);
  const groups = groupSandboxApps(apps, query);
  const shown = groups.flatMap((group) => group.apps);
  // What was typed may have left fewer rows than the highlight had reached.
  const activeId = searching ? shown[Math.min(cursor, shown.length - 1)]?.id : undefined;

  const onSearchKey = (event: React.KeyboardEvent<HTMLInputElement>) => {
    // The form's own chord: not this field's Enter.
    if (isSubmitChord(event.nativeEvent)) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setCursor((at) =>
        Math.max(0, Math.min(shown.length - 1, Math.min(at, shown.length - 1) + step))
      );
    } else if (event.key === "Enter") {
      // Never the form's implicit submit. Only a bare Enter picks: a modified one is a chord
      // meant for something else.
      event.preventDefault();
      const bare = !(event.metaKey || event.ctrlKey || event.altKey || event.shiftKey);
      if (!activeId || !bare) return;
      const checked = pickedIds.includes(activeId);
      if (checked || !full) onToggle(activeId, !checked);
    }
  };

  return (
    // A column that can be squeezed: the search keeps its height and the list gives up its own.
    <div className='flex min-h-0 flex-col'>
      <div className='relative shrink-0'>
        <Search
          className='pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground'
          aria-hidden
        />
        <Input
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setCursor(0);
          }}
          onKeyDown={onSearchKey}
          onFocus={() => setSearching(true)}
          onBlur={() => setSearching(false)}
          placeholder='Search apps or organizations'
          aria-label='Search apps or organizations'
          aria-describedby={keysHintId}
          autoComplete='off'
          className='pl-9'
          data-testid='account-token-sandbox-search'
        />
      </div>
      <div
        className='mt-1 max-h-64 min-h-0 overflow-y-auto'
        data-testid='account-token-sandbox-picker'
      >
        {groups.length === 0 && (
          <p
            className='px-3 py-2 text-muted-foreground'
            data-testid='account-token-sandbox-no-match'
          >
            No app or organization matches "{query.trim()}".
          </p>
        )}
        {groups.map((group) => (
          <div
            key={group.orgId}
            data-testid='account-token-sandbox-org'
            data-org-name={group.orgName}
          >
            <p className='flex h-7 items-end px-3 pb-1 font-medium text-muted-foreground text-xs'>
              <span className='truncate'>{group.orgName}</span>
            </p>
            <ul>
              {group.apps.map((app) => {
                const checked = pickedIds.includes(app.id);
                return (
                  <AppRow
                    key={app.id}
                    app={app}
                    checked={checked}
                    disabled={full && !checked}
                    active={app.id === activeId}
                    onToggle={(on) => onToggle(app.id, on)}
                  />
                );
              })}
            </ul>
          </div>
        ))}
      </div>
    </div>
  );
};

export default SandboxAppPicker;
