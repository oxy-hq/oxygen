import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import type { AppSecretNeeds } from "@/types/sharedSecrets";

/** More apps than this and the list gets a search box. */
const SEARCH_FROM = 7;

/**
 * Pick the apps that read one secret. Each app says why it might matter: it
 * **needs** this key (its build declares it and nothing provides it yet), or it
 * **has its own value**, which wins over a shared one — sharing still works, the
 * app just keeps reading its own until that is deleted.
 */
export const AppChecklist = ({
  apps,
  secretName,
  selected,
  onChange,
  ownValue = new Set(),
  locked = new Set(),
  lockedReason
}: {
  apps: AppSecretNeeds[];
  secretName: string;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  /** Apps that store their own value of `secretName`. */
  ownValue?: Set<string>;
  /**
   * Apps that must stay selected — the caller would take the secret away from
   * them otherwise. Shown ticked and disabled, and left alone by Select all /
   * Clear, so the list cannot express a choice the caller will not honour.
   */
  locked?: Set<string>;
  /** Why those are fixed, as a tooltip. */
  lockedReason?: string;
}) => {
  const [query, setQuery] = useState("");
  const name = secretName.trim();
  const q = query.trim().toLowerCase();
  const shown = apps.filter((a) => !q || a.app_name.toLowerCase().includes(q));

  if (apps.length === 0) {
    return <p className='text-muted-foreground text-xs'>This workspace has no custom apps yet.</p>;
  }

  const toggle = (id: string, on: boolean) => {
    if (locked.has(id)) return;
    const next = new Set(selected);
    if (on) next.add(id);
    else next.delete(id);
    onChange(next);
  };

  // Select all / none acts on what is on screen, so it means what it says
  // while a search is narrowing the list. A locked app is never cleared.
  const toggleable = shown.filter((a) => !locked.has(a.app_id));
  const allShown = toggleable.length > 0 && toggleable.every((a) => selected.has(a.app_id));
  const toggleShown = () => {
    const next = new Set(selected);
    for (const app of toggleable) {
      if (allShown) next.delete(app.app_id);
      else next.add(app.app_id);
    }
    onChange(next);
  };

  return (
    <div className='flex flex-col gap-2'>
      {apps.length >= SEARCH_FROM && (
        <Input
          placeholder='Find an app'
          aria-label='Find an app'
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className='h-8'
        />
      )}
      <div className='flex items-center justify-between'>
        <span className='text-muted-foreground text-xs' aria-live='polite'>
          {new Set([...selected, ...locked]).size} of {apps.length} selected
        </span>
        {toggleable.length > 1 && (
          <Button
            type='button'
            variant='ghost'
            size='sm'
            className='h-6 px-2 text-xs'
            onClick={toggleShown}
            data-testid='secret-app-checklist-toggle-all'
          >
            {allShown ? "Clear" : "Select all"}
          </Button>
        )}
      </div>
      <ul
        className='flex max-h-56 flex-col overflow-y-auto rounded-md border'
        data-testid='secret-app-checklist'
      >
        {shown.map((app) => {
          const needs = app.missing.some((m) => m.key === name);
          const isLocked = locked.has(app.app_id);
          const checkboxId = `share-${app.app_id}`;
          return (
            <li
              key={app.app_id}
              className='flex items-center gap-2 border-b px-3 py-2 last:border-0'
            >
              <Checkbox
                id={checkboxId}
                checked={isLocked || selected.has(app.app_id)}
                disabled={isLocked}
                onCheckedChange={(on) => toggle(app.app_id, on === true)}
                aria-label={app.app_name}
                title={isLocked ? lockedReason : undefined}
              />
              <label
                htmlFor={checkboxId}
                className={`flex min-w-0 flex-1 flex-col ${isLocked ? "cursor-default" : "cursor-pointer"}`}
                title={isLocked ? lockedReason : undefined}
              >
                <span className='truncate text-sm'>{app.app_name}</span>
                <span className='truncate font-mono text-muted-foreground text-xs'>
                  {app.app_slug}
                </span>
              </label>
              {needs && (
                <Badge variant='outline' className='border-warning/40 text-status-warning-text'>
                  Needs it
                </Badge>
              )}
              {ownValue.has(app.app_id) && (
                <Badge
                  variant='outline'
                  className='font-normal text-muted-foreground'
                  title={`This app stores its own ${name}, which it reads instead of a shared one.`}
                >
                  Has its own value
                </Badge>
              )}
            </li>
          );
        })}
        {shown.length === 0 && (
          <li className='px-3 py-2 text-muted-foreground text-xs'>No app matches.</li>
        )}
      </ul>
    </div>
  );
};
