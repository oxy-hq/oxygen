import type React from "react";

import { coversEvery } from "@/components/airway/ResetCursorsButton/selection";
import { Checkbox } from "@/components/ui/shadcn/checkbox";

/**
 * Which resources to rewind.
 *
 * The names come from `GET /resource-cursors` — the raw keys of the pipeline's
 * stored `resource_states`, which is exactly the name space `/reset-cursors`
 * accepts. They are deliberately not derived from a run's lineage: those are
 * normalized *table* names, and where the two diverge the reset clears nothing
 * and only says so afterwards.
 *
 * Nothing is selected by default. On a pipeline like BMG's `amazon_vc`, "every
 * resource" is precisely the choice an append-only sibling refuses, so a
 * default of everything makes an operator's first click a refusal — and an
 * operator whose first click is a refusal is an operator reaching for the
 * override in their first thirty seconds.
 *
 * ## The every-resource row
 *
 * A shortcut for ticking every box, and it **checks itself when every box
 * below is ticked**: the same intent, shown the same way. Either route sends
 * the enumerated names, never the route's `[]` — see `index.tsx`. The server
 * judges a list naming every held cursor as the whole pipeline, so a
 * whole-pipeline rewind of a connector that renames nested children is not
 * refused for scoping (a refusal that routine would train the override).
 */
const ResourcePicker: React.FC<{
  resources: string[];
  selected: string[];
  onToggle: (resource: string) => void;
  /** Tick the every-resource row: select all, or clear back to nothing. */
  onSelectEvery: (every: boolean) => void;
  disabled?: boolean;
}> = ({ resources, selected, onToggle, onSelectEvery, disabled = false }) => {
  if (resources.length === 0) {
    return (
      <p data-testid='airway-cursor-picker-empty' className='text-muted-foreground text-xs'>
        This pipeline holds no cursors, so there is nothing to rewind. A pipeline that has never run
        starts from its <span className='font-mono'>default_start</span> already.
      </p>
    );
  }

  // A set test, not a count: `resources` can change under a standing
  // selection, and equal lengths would then tick this row for a selection
  // that does not contain every resource.
  const everySelected = coversEvery(resources, selected);

  return (
    <div className='space-y-2'>
      <span className='font-medium text-muted-foreground text-xs uppercase tracking-wide'>
        Resources to rewind
      </span>

      <div className='flex items-center gap-2 rounded border border-border px-2 py-1.5'>
        <Checkbox
          id='airway-cursor-every-resource'
          checked={everySelected}
          onCheckedChange={(next) => onSelectEvery(next === true)}
          disabled={disabled}
          aria-label='Every resource that holds a cursor'
          data-testid='airway-cursor-every-resource'
        />
        <label htmlFor='airway-cursor-every-resource' className='cursor-pointer text-xs'>
          Every resource that holds a cursor{" "}
          <span className='text-muted-foreground'>({resources.length})</span>
        </label>
      </div>

      <ul data-testid='airway-cursor-picker' className='max-h-48 space-y-1 overflow-auto'>
        {resources.map((resource) => (
          <li key={resource} className='flex items-center gap-2 rounded px-1 py-1 hover:bg-muted'>
            {/* Explicit `htmlFor`, not a wrapping label: Radix renders the
                checkbox as a `button`, so nesting associates nothing. */}
            <Checkbox
              id={`airway-cursor-resource-${resource}`}
              checked={selected.includes(resource)}
              onCheckedChange={() => onToggle(resource)}
              disabled={disabled}
              aria-label={resource}
            />
            <label
              htmlFor={`airway-cursor-resource-${resource}`}
              className='cursor-pointer font-mono text-xs'
            >
              {resource}
            </label>
          </li>
        ))}
      </ul>
    </div>
  );
};

export default ResourcePicker;
