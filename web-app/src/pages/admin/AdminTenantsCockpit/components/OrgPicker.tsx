import { ChevronsUpDown } from "lucide-react";
import { useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/shadcn/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { useAdminOrgsList } from "@/hooks/api/adminTenants/index";
import type { AdminOrgMeta } from "@/services/api/adminTenants";

/** Combobox that searches organizations and returns the picked one. Reused for
 *  attach-to-partner, transfer-workspace-org, and grant-partnership. */
export default function OrgPicker({
  onPick,
  exclude = [],
  label = "Select organization…",
  disabled
}: {
  onPick: (org: AdminOrgMeta) => void;
  exclude?: string[];
  label?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const { data } = useAdminOrgsList({ search: q || undefined });
  const orgs = (data ?? []).filter((o) => !exclude.includes(o.id));

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant='outline' size='sm' disabled={disabled} className='justify-between gap-2'>
          {label}
          <ChevronsUpDown className='size-3.5 opacity-50' />
        </Button>
      </PopoverTrigger>
      <PopoverContent className={`${DESIGN_REFERENCE} w-72 p-0`} align='start'>
        <Command shouldFilter={false}>
          <CommandInput value={q} onValueChange={setQ} placeholder='Search organizations…' />
          <CommandList>
            <CommandEmpty>No organizations.</CommandEmpty>
            <CommandGroup>
              {orgs.map((o) => (
                <CommandItem
                  key={o.id}
                  value={o.id}
                  onSelect={() => {
                    onPick(o);
                    setOpen(false);
                    setQ("");
                  }}
                >
                  <div className='min-w-0'>
                    <div className='truncate text-xs'>{o.name}</div>
                    <div className='truncate text-muted-foreground text-xs'>{o.slug}</div>
                  </div>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
