import { Filter, RotateCcw, X } from "lucide-react";
import { useRef, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import { InstancePickerPopover } from "@/pages/ide/WorldModel/components/InstancePickerPopover";
import type { WmInstance } from "@/types/worldModel";
import { PRESET_DAYS } from "./periodPresets";
import { EMPTY_SCENARIO, type ScenarioState, type ScenarioUpdate } from "./scenarioUrl";

interface ScenarioToolbarProps {
  state: ScenarioState;
  /** Takes an updater so a control committing in the same tick as one of the
   *  scenario effects cannot revert that effect's field — see
   *  `ScenarioUpdate`. */
  onChange: (next: ScenarioUpdate) => void;
  /** Time dimensions available in this layer, from useTimeDimensions(). */
  timeDimensions: string[];
  /** The pinned dimension when no lever's view declares it, else `null`.
   *
   *  Computed by the parent, not here, because the same predicate decides
   *  whether the QUERY carries it — and the two must not be able to disagree.
   *  It still has to render: a select silently showing a placeholder while
   *  the scenario runs on the old value is worse than showing the bad value,
   *  so it is appended to the options and called out underneath. */
  foreign: string | null;
}

interface PendingPicker {
  entityId: string;
  entityLabel: string;
  position: { x: number; y: number };
}

/**
 * Time dimension · period preset · scope · Reset, in one row above the
 * canvas. When the layer has no time dimension at all, the period and scope
 * controls have nothing meaningful to offer (there is no window to size a
 * baseline over), so they are replaced by a single explanatory line rather
 * than shown disabled — the mode itself still works in delta-only form.
 */
export function ScenarioToolbar({
  state,
  onChange,
  timeDimensions,
  foreign
}: ScenarioToolbarProps) {
  const { data: model } = useWorldModel();
  const scopeTriggerRef = useRef<HTMLButtonElement>(null);
  const [pendingPicker, setPendingPicker] = useState<PendingPicker | null>(null);

  const scopedEntity = state.instance
    ? model?.entities.find((e) => e.id === state.instance?.entity)
    : undefined;

  function openPickerFor(entityId: string, entityLabel: string) {
    const rect = scopeTriggerRef.current?.getBoundingClientRect();
    setPendingPicker({
      entityId,
      entityLabel,
      position: { x: rect?.left ?? 0, y: rect?.bottom ?? 0 }
    });
  }

  const options = foreign ? [...timeDimensions, foreign] : timeDimensions;

  function handlePickInstance(instance: WmInstance) {
    if (!pendingPicker) return;
    onChange((prev) => ({
      ...prev,
      instance: { entity: pendingPicker.entityId, key: instance.key }
    }));
    setPendingPicker(null);
  }

  return (
    <div
      className='flex items-center gap-2 border-border border-b bg-card px-4 py-2'
      data-testid='scenario-toolbar'
    >
      {options.length === 0 ? (
        <p className='text-muted-foreground text-xs'>
          No time dimension in this layer — levers propagate as relative changes only.
        </p>
      ) : (
        <>
          <Select
            value={state.timeDimension ?? undefined}
            onValueChange={(v) => onChange((prev) => ({ ...prev, timeDimension: v }))}
          >
            <SelectTrigger
              size='sm'
              aria-label='Time dimension'
              className='h-7 w-48 font-mono text-xs'
            >
              <SelectValue placeholder='Time dimension' />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {options.map((dim) => (
                <SelectItem key={dim} value={dim} className='font-mono text-xs'>
                  {dim}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {foreign && (
            <p
              className='max-w-96 text-destructive text-xs'
              data-testid='scenario-foreign-time-dim'
            >
              <span className='font-mono'>{foreign}</span> isn't on a pinned lever's view. Grouping
              by it joins that view in on whatever key they share, which flattens every measure
              across dates — pick one above.
            </p>
          )}

          <Select
            value={String(state.periodDays)}
            onValueChange={(v) => onChange((prev) => ({ ...prev, periodDays: Number(v) }))}
          >
            <SelectTrigger size='sm' aria-label='Period' className='h-7 w-28 font-mono text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {PRESET_DAYS.map((days) => (
                <SelectItem key={days} value={String(days)} className='font-mono text-xs'>
                  Last {days}d
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {state.instance ? (
            <div className='flex items-center gap-1.5 border border-info/50 bg-info/5 px-2 py-1 font-mono text-xs'>
              <span className='text-muted-foreground'>scope</span>
              <span className='text-info'>{scopedEntity?.label ?? state.instance.entity}</span>
              <span className='text-muted-foreground'>·</span>
              <span className='text-foreground'>{state.instance.key}</span>
              <button
                type='button'
                onClick={() => onChange((prev) => ({ ...prev, instance: null }))}
                className='text-muted-foreground hover:text-foreground'
                aria-label='Clear scope'
              >
                <X size={10} />
              </button>
            </div>
          ) : (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  ref={scopeTriggerRef}
                  variant='outline'
                  size='sm'
                  className='h-7 gap-1 text-xs'
                >
                  <Filter size={12} />
                  Scope
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align='start' className={DESIGN_REFERENCE}>
                {(model?.entities.length ?? 0) === 0 ? (
                  <DropdownMenuItem disabled>No entities in this layer</DropdownMenuItem>
                ) : (
                  model?.entities.map((entity) => (
                    <DropdownMenuItem
                      key={entity.id}
                      onSelect={() => openPickerFor(entity.id, entity.label)}
                    >
                      {entity.label}
                    </DropdownMenuItem>
                  ))
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </>
      )}

      <div className='ml-auto'>
        <Button
          variant='ghost'
          size='sm'
          className='h-7 gap-1 text-xs'
          onClick={() => onChange(EMPTY_SCENARIO)}
        >
          <RotateCcw size={12} />
          Reset
        </Button>
      </div>

      {pendingPicker && (
        <InstancePickerPopover
          entityId={pendingPicker.entityId}
          entityLabel={pendingPicker.entityLabel}
          position={pendingPicker.position}
          onPick={handlePickInstance}
          onClose={() => setPendingPicker(null)}
        />
      )}
    </div>
  );
}
