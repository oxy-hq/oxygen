import { Search } from "lucide-react";
import type React from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";

export interface JobFilters {
  type: "all" | "dag" | "elt";
  state: "all" | "enabled" | "disabled";
  health: "all" | "healthy" | "error";
  search: string;
}

export const DEFAULT_JOB_FILTERS: JobFilters = {
  type: "all",
  state: "all",
  health: "all",
  search: ""
};

const FilterSelect = <T extends string>({
  value,
  onChange,
  options,
  width
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  width: string;
}) => (
  <Select value={value} onValueChange={(v) => onChange(v as T)}>
    <SelectTrigger className={width} size='sm'>
      <SelectValue />
    </SelectTrigger>
    <SelectContent className={DESIGN_REFERENCE}>
      {options.map((o) => (
        <SelectItem key={o.value} value={o.value}>
          {o.label}
        </SelectItem>
      ))}
    </SelectContent>
  </Select>
);

/** Filter + search bar for the job catalog. */
export const JobsFilterBar: React.FC<{
  value: JobFilters;
  onChange: (next: JobFilters) => void;
}> = ({ value, onChange }) => {
  const patch = (p: Partial<JobFilters>) => onChange({ ...value, ...p });

  return (
    <div className='flex flex-wrap items-center gap-2'>
      <FilterSelect
        value={value.type}
        onChange={(type) => patch({ type })}
        width='w-32'
        options={[
          { value: "all", label: "All types" },
          { value: "dag", label: "DAG automation" },
          { value: "elt", label: "ELT pipeline" }
        ]}
      />
      <FilterSelect
        value={value.state}
        onChange={(state) => patch({ state })}
        width='w-32'
        options={[
          { value: "all", label: "All states" },
          { value: "enabled", label: "Enabled" },
          { value: "disabled", label: "Disabled" }
        ]}
      />
      <FilterSelect
        value={value.health}
        onChange={(health) => patch({ health })}
        width='w-32'
        options={[
          { value: "all", label: "All health" },
          { value: "healthy", label: "Healthy" },
          { value: "error", label: "Has error" }
        ]}
      />
      <div className='relative ml-auto'>
        <Search className='absolute top-1/2 left-2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground' />
        <Input
          value={value.search}
          onChange={(e) => patch({ search: e.target.value })}
          placeholder='Search jobs'
          className='h-8 w-52 pl-7'
        />
      </div>
    </div>
  );
};
