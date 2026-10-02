import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { toText } from "@/libs/utils/string";
import type { ControlConfig, DataContainer } from "@/types/app";
import { DateControl } from "./DateControl";
import { SelectControl } from "./SelectControl";
import { ToggleControl } from "./ToggleControl";

type Props = {
  controls: ControlConfig[];
  values: Record<string, unknown>;
  data?: DataContainer;
  onChange: (name: string, value: unknown) => void;
  onRun?: () => void;
  isRunning?: boolean;
};

export function ControlsBar({ controls, values, data, onChange, onRun, isRunning }: Props) {
  if (controls.length === 0) return null;

  return (
    <div className='flex flex-wrap items-end gap-3 py-2'>
      {controls.map((control) => {
        const value = values[control.name];

        if (control.type === "toggle") {
          return (
            <ToggleControl
              key={control.name}
              control={control}
              value={Boolean(value)}
              onChange={(v) => onChange(control.name, v)}
            />
          );
        }

        if (control.type === "date") {
          return (
            <DateControl
              key={control.name}
              control={control}
              value={toText(value)}
              onChange={(v) => onChange(control.name, v)}
            />
          );
        }

        // default: select
        return (
          <SelectControl
            key={control.name}
            control={control}
            value={toText(value)}
            data={data}
            onChange={(v) => onChange(control.name, v)}
          />
        );
      })}

      {onRun && (
        <Button
          variant='default'
          content='icon'
          onClick={onRun}
          disabled={isRunning}
          className='ml-auto shrink-0'
        >
          {isRunning ? <Spinner /> : <RefreshCw />}
        </Button>
      )}
    </div>
  );
}
