import { LayoutGrid, Rows3, Search } from "lucide-react";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Switch } from "@/components/ui/shadcn/switch";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { StatusFilter, TraceView } from "../types";

interface TracesToolbarProps {
  search: string;
  onSearchChange: (value: string) => void;
  status: StatusFilter;
  onStatusChange: (value: StatusFilter) => void;
  live: boolean;
  onLiveChange: (value: boolean) => void;
  view: TraceView;
  onViewChange: (value: TraceView) => void;
}

/**
 * Traces toolbar (Theme 3): free-text search + status filter (3a), a live-tail
 * auto-refresh switch (3c), and the card/table view toggle (3d).
 */
export function TracesToolbar({
  search,
  onSearchChange,
  status,
  onStatusChange,
  live,
  onLiveChange,
  view,
  onViewChange
}: TracesToolbarProps) {
  return (
    <div className='mb-3 flex flex-wrap items-center gap-2'>
      <div className='relative min-w-48 flex-1'>
        <Search className='absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground' />
        <Input
          value={search}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder='Search span, agent, prompt, or trace id…'
          className='pl-8'
          aria-label='Search traces'
        />
      </div>

      <Select value={status} onValueChange={(v) => onStatusChange(v as StatusFilter)}>
        <SelectTrigger className='w-36' data-testid='traces-status-filter'>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value='all'>All statuses</SelectItem>
          <SelectItem value='ok'>Success</SelectItem>
          <SelectItem value='error'>Errors</SelectItem>
        </SelectContent>
      </Select>

      <div className='flex items-center gap-2 rounded-md border px-2.5 py-1.5'>
        <Switch id='live-tail' checked={live} onCheckedChange={onLiveChange} />
        <Label htmlFor='live-tail' className='flex cursor-pointer items-center gap-1.5 text-sm'>
          {live && (
            <span className='relative flex size-2'>
              <span className='absolute inline-flex size-full animate-ping rounded-full bg-success opacity-75' />
              <span className='relative inline-flex size-2 rounded-full bg-success' />
            </span>
          )}
          Live
        </Label>
      </div>

      <ToggleGroup
        type='single'
        value={view}
        onValueChange={(v) => v && onViewChange(v as TraceView)}
        variant='outline'
        size='sm'
      >
        <ToggleGroupItem value='card' aria-label='Card view'>
          <LayoutGrid className='size-4' />
        </ToggleGroupItem>
        <ToggleGroupItem value='table' aria-label='Table view'>
          <Rows3 className='size-4' />
        </ToggleGroupItem>
      </ToggleGroup>
    </div>
  );
}
