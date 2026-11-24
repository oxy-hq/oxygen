import { ListFilter } from "lucide-react";
import { useState } from "react";
import { Sheet, SheetContent, SheetTitle, SheetTrigger } from "@/components/ui/shadcn/sheet";
import type { InboxFilter, NotificationCatalog, NotificationStream } from "@/types/notifications";
import { CategoryTree } from "./CategoryTree";

interface FilterSheetProps {
  catalog: NotificationCatalog | undefined;
  isPending: boolean;
  isError: boolean;
  filter: InboxFilter;
  /** The current scope, in words: what the button says. */
  label: string;
  onSelect: (filter: InboxFilter) => void;
  stream?: NotificationStream;
}

/**
 * On a phone the category tree is a sheet: a button naming the current scope
 * opens it, and picking a node narrows the list and closes it.
 */
export function FilterSheet({
  catalog,
  isPending,
  isError,
  filter,
  label,
  onSelect,
  stream
}: FilterSheetProps) {
  const [open, setOpen] = useState(false);
  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>
        <button
          type='button'
          data-testid='notifications-filter-open'
          className='flex min-w-0 flex-1 items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-left text-sm'
        >
          <ListFilter className='size-4 shrink-0 text-muted-foreground' />
          <span className='truncate'>{label}</span>
        </button>
      </SheetTrigger>
      <SheetContent
        side='bottom'
        aria-describedby={undefined}
        className='inset-0 h-dvh gap-0 border-0 p-0'
        data-testid='notifications-filter-sheet'
      >
        <div className='flex h-12 shrink-0 items-center border-b pr-12 pl-4'>
          <SheetTitle className='text-base'>Filter</SheetTitle>
        </div>
        <div className='min-h-0 flex-1 overflow-y-auto p-2'>
          <CategoryTree
            catalog={catalog}
            isPending={isPending}
            isError={isError}
            filter={filter}
            stream={stream}
            onSelect={(next) => {
              onSelect(next);
              setOpen(false);
            }}
          />
        </div>
      </SheetContent>
    </Sheet>
  );
}
