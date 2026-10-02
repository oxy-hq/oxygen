import { Plus } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { toText } from "@/libs/utils/string";
import type { SemanticQueryFilter } from "@/services/api/semantic";
import CollapsibleSection from "./CollapsibleSection";
import FilterRow, { type FilterDimension } from "./FilterRow";

const formatFilterValue = (f: SemanticQueryFilter): string => {
  if ("relative" in f && f.relative) return String(f.relative);
  if ("from" in f || "to" in f) {
    // Bounds are ISO or relative strings; the type also admits a Date, which prints as ISO.
    const from = "from" in f ? toText(f.from ?? "…") : "…";
    const to = "to" in f ? toText(f.to ?? "…") : "…";
    return `${from} → ${to}`;
  }
  if ("values" in f && Array.isArray(f.values)) return (f.values as unknown[]).join(", ");
  if ("value" in f && f.value != null)
    return Array.isArray(f.value) ? f.value.join(", ") : String(f.value);
  return "";
};

interface FiltersDisplayProps {
  filters: SemanticQueryFilter[];
  editable?: boolean;
  availableDimensions?: FilterDimension[];
  onAddFilter?: () => void;
  onUpdateFilter?: (index: number, updates: SemanticQueryFilter) => void;
  onRemoveFilter?: (index: number) => void;
}

const FiltersDisplay = ({
  filters,
  editable = false,
  availableDimensions = [],
  onAddFilter,
  onUpdateFilter,
  onRemoveFilter
}: FiltersDisplayProps) => {
  if (!editable && filters.length === 0) return null;

  return (
    <CollapsibleSection title='Filters' count={filters.length}>
      {editable && onUpdateFilter && onRemoveFilter ? (
        <div className='flex flex-col gap-2'>
          {filters.map((filter, index) => (
            <FilterRow
              key={`${filter.field}-${filter.op}-${index}`}
              filter={filter}
              availableDimensions={availableDimensions}
              onUpdate={(updates) => onUpdateFilter(index, updates)}
              onRemove={() => onRemoveFilter(index)}
            />
          ))}
          {onAddFilter && (
            <Button variant='ghost' size='sm' className='w-fit' onClick={onAddFilter}>
              <Plus />
              Add filter
            </Button>
          )}
        </div>
      ) : (
        <div className='flex flex-wrap gap-1.5'>
          {filters.map((f, i) => (
            <span
              key={`filter-${f.field}-${f.op}-${i}`}
              className='inline-flex items-center gap-1 rounded-md bg-muted px-2 py-0.5 text-xs'
            >
              <span className='font-medium'>{f.field.split(".").pop()}</span>
              <span className='text-muted-foreground'>{f.op}</span>
              <span>{formatFilterValue(f)}</span>
            </span>
          ))}
        </div>
      )}
    </CollapsibleSection>
  );
};

export default FiltersDisplay;
