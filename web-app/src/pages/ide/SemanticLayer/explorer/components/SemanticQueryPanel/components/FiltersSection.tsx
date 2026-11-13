import FilterRow from "@/components/SemanticQueryPanel/FilterRow";
import type { Filter } from "../../../types";

interface FiltersSectionProps {
  filters: Filter[];
  availableDimensions: { name: string; fullName: string; type?: string }[];
  onUpdateFilter: (index: number, updates: Filter) => void;
  onRemoveFilter: (index: number) => void;
}

const FiltersSection = ({
  filters,
  availableDimensions,
  onUpdateFilter,
  onRemoveFilter
}: FiltersSectionProps) => {
  if (filters.length === 0) return null;

  return (
    <div className='space-y-2 border-b p-3'>
      <p className='font-medium text-muted-foreground text-xs uppercase tracking-wider'>Filters</p>
      {filters.map((filter, index) => (
        <FilterRow
          key={index}
          filter={filter}
          availableDimensions={availableDimensions}
          onUpdate={(updates) => onUpdateFilter(index, updates)}
          onRemove={() => onRemoveFilter(index)}
        />
      ))}
    </div>
  );
};

export default FiltersSection;
