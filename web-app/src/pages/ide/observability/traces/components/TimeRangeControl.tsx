import { CalendarClock } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { DURATION_VALUES, type DurationValue, type TimeRange } from "../types";

interface TimeRangeControlProps {
  value: TimeRange;
  onChange: (range: TimeRange) => void;
}

// epoch seconds -> value string for <input type="datetime-local"> (local wall clock)
function toLocalInput(epochSec: number): string {
  const d = new Date(epochSec * 1000);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fromLocalInput(value: string): number | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isNaN(ms) ? null : Math.floor(ms / 1000);
}

function formatRangeLabel(from: number, to: number): string {
  const fmt = (s: number) =>
    new Date(s * 1000).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    });
  return `${fmt(from)} – ${fmt(to)}`;
}

/**
 * Time-window control (Theme 3b): rolling presets (1h…90d) plus an absolute
 * from/to range picked in a popover. Selecting a preset clears the custom range
 * and vice-versa — the two are mutually exclusive.
 */
export function TimeRangeControl({ value, onChange }: TimeRangeControlProps) {
  const isCustom = value.kind === "custom";
  const [open, setOpen] = useState(false);
  const nowSec = () => Math.floor(Date.now() / 1000);
  const [draftFrom, setDraftFrom] = useState(() =>
    isCustom ? toLocalInput(value.from) : toLocalInput(nowSec() - 3600)
  );
  const [draftTo, setDraftTo] = useState(() =>
    isCustom ? toLocalInput(value.to) : toLocalInput(nowSec())
  );

  const from = fromLocalInput(draftFrom);
  const to = fromLocalInput(draftTo);
  const invalid = from === null || to === null || from >= to;

  const applyCustom = () => {
    if (invalid) return;
    onChange({ kind: "custom", from, to });
    setOpen(false);
  };

  return (
    <div className='flex items-center gap-2'>
      <Tabs
        value={isCustom ? "" : value.value}
        onValueChange={(v) => v && onChange({ kind: "preset", value: v as DurationValue })}
      >
        <TabsList>
          {DURATION_VALUES.map((duration) => (
            <TabsTrigger key={duration} value={duration}>
              {duration}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            variant={isCustom ? "secondary" : "outline"}
            size='sm'
            className='gap-1.5'
            title='Pick an absolute time range'
          >
            <CalendarClock className='size-3.5' />
            {isCustom ? formatRangeLabel(value.from, value.to) : "Custom"}
          </Button>
        </PopoverTrigger>
        <PopoverContent align='end' className='w-72 space-y-3'>
          <div className='space-y-1.5'>
            <Label htmlFor='range-from' className='text-muted-foreground text-xs'>
              From
            </Label>
            <Input
              id='range-from'
              type='datetime-local'
              value={draftFrom}
              max={draftTo || undefined}
              onChange={(e) => setDraftFrom(e.target.value)}
              className='text-xs'
            />
          </div>
          <div className='space-y-1.5'>
            <Label htmlFor='range-to' className='text-muted-foreground text-xs'>
              To
            </Label>
            <Input
              id='range-to'
              type='datetime-local'
              value={draftTo}
              min={draftFrom || undefined}
              onChange={(e) => setDraftTo(e.target.value)}
              className='text-xs'
            />
          </div>
          {invalid && (
            <p className='text-destructive text-xs'>Pick a start that is before the end.</p>
          )}
          <Button size='sm' className='w-full' onClick={applyCustom} disabled={invalid}>
            Apply range
          </Button>
        </PopoverContent>
      </Popover>
    </div>
  );
}
