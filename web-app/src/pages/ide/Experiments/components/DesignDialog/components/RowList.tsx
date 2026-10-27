import { Plus, X } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { FieldError } from "@/components/ui/shadcn/field";

/** A repeatable list of rows with add and remove, and one message about the list as a whole. */
export default function RowList({
  id,
  title,
  empty,
  addLabel,
  keys,
  error,
  onAdd,
  onRemove,
  renderRow
}: {
  id: string;
  title: string;
  empty: string;
  addLabel: string;
  /** The field array's own ids, one per row, in order. */
  keys: string[];
  error?: string;
  onAdd: () => void;
  onRemove: (index: number) => void;
  renderRow: (index: number) => ReactNode;
}) {
  return (
    <div className='flex flex-col gap-2' data-testid={id}>
      <span className='font-medium text-sm'>{title}</span>
      {keys.length === 0 && <p className='text-muted-foreground text-sm'>{empty}</p>}
      {keys.map((key, i) => (
        <div key={key} className='flex items-start gap-2' data-testid={`${id}-${i}`}>
          <div className='flex-1'>{renderRow(i)}</div>
          <Button
            type='button'
            variant='ghost'
            size='sm'
            className='mt-6'
            aria-label={`Remove row ${i + 1}`}
            onClick={() => onRemove(i)}
          >
            <X className='h-4 w-4' />
          </Button>
        </div>
      ))}
      {error && <FieldError data-testid={`${id}-error`}>{error}</FieldError>}
      <div>
        <Button type='button' variant='outline' size='sm' onClick={onAdd} data-testid={`${id}-add`}>
          <Plus className='h-4 w-4' />
          {addLabel}
        </Button>
      </div>
    </div>
  );
}
