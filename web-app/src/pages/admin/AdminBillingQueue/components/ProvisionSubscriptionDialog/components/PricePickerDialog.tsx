import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/shadcn/command";
import type { AdminPriceDto } from "@/services/api/billing";

interface PricePickerDialogProps {
  open: boolean;
  prices: AdminPriceDto[];
  excludePriceIds: string[];
  onPick: (price: AdminPriceDto) => void;
  onOpenChange: (open: boolean) => void;
}

const UNGROUPED = "Other";

function groupPrices(prices: AdminPriceDto[]): Array<[string, AdminPriceDto[]]> {
  const groups = new Map<string, AdminPriceDto[]>();
  for (const p of prices) {
    const key = p.product_name ?? UNGROUPED;
    const list = groups.get(key) ?? [];
    list.push(p);
    groups.set(key, list);
  }
  return Array.from(groups.entries());
}

export function PricePickerDialog({
  open,
  prices,
  excludePriceIds,
  onPick,
  onOpenChange
}: PricePickerDialogProps) {
  const excluded = new Set(excludePriceIds);
  const available = prices.filter((p) => !excluded.has(p.id));
  const groups = groupPrices(available);

  return (
    <CommandDialog
      className={DESIGN_REFERENCE}
      open={open}
      onOpenChange={onOpenChange}
      title='Pick a price'
      description='Search Stripe prices by product, nickname, or amount.'
    >
      <CommandInput placeholder='Search prices…' />
      <CommandList>
        <CommandEmpty>
          {prices.length === 0
            ? "No active recurring per-unit or graduated-tiered prices on the Stripe account."
            : available.length === 0
              ? "All prices already added."
              : "No matching prices."}
        </CommandEmpty>
        {groups.map(([productName, items]) => (
          <CommandGroup key={productName} heading={productName}>
            {items.map((price) => (
              <CommandItem
                key={price.id}
                value={`${productName} ${price.nickname ?? ""} ${price.amount_display} ${price.interval}`}
                onSelect={() => {
                  onPick(price);
                  onOpenChange(false);
                }}
              >
                <div className='flex w-full items-center justify-between gap-3'>
                  <span className='truncate'>{price.nickname ?? "Untitled price"}</span>
                  <span className='shrink-0 text-muted-foreground text-xs tabular-nums'>
                    {price.amount_display} / {price.interval}
                  </span>
                </div>
              </CommandItem>
            ))}
          </CommandGroup>
        ))}
      </CommandList>
    </CommandDialog>
  );
}
