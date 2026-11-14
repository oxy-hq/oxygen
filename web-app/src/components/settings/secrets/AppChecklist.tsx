import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
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
  ownValue = new Set()
}: {
  apps: AppSecretNeeds[];
  secretName: string;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  /** Apps that store their own value of `secretName`. */
  ownValue?: Set<string>;
}) => {
  const [query, setQuery] = useState("");
  const name = secretName.trim();
  const q = query.trim().toLowerCase();
  const shown = apps.filter((a) => !q || a.app_name.toLowerCase().includes(q));

  if (apps.length === 0) {
    return <p className='text-muted-foreground text-xs'>This workspace has no custom apps yet.</p>;
  }

  const toggle = (id: string, on: boolean) => {
    const next = new Set(selected);
    if (on) next.add(id);
    else next.delete(id);
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
      <ul
        className='flex max-h-56 flex-col overflow-y-auto rounded-md border'
        data-testid='secret-app-checklist'
      >
        {shown.map((app) => {
          const needs = app.missing.some((m) => m.key === name);
          const checkboxId = `share-${app.app_id}`;
          return (
            <li
              key={app.app_id}
              className='flex items-center gap-2 border-b px-3 py-2 last:border-0'
            >
              <Checkbox
                id={checkboxId}
                checked={selected.has(app.app_id)}
                onCheckedChange={(on) => toggle(app.app_id, on === true)}
                aria-label={app.app_name}
              />
              <label htmlFor={checkboxId} className='flex min-w-0 flex-1 cursor-pointer flex-col'>
                <span className='truncate text-sm'>{app.app_name}</span>
                <span className='truncate font-mono text-muted-foreground text-xs'>
                  {app.app_slug}
                </span>
              </label>
              {needs && (
                <Badge variant='outline' className='border-warning/40 text-warning'>
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
