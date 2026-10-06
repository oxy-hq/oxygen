import { Bot, User } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { TokenOwner } from "@/types/apiToken";

/** Who holds a credential: a person, or one of the org's service accounts (shown by its handle). */
export function InventoryOwner({ owner }: { owner: TokenOwner }) {
  const isAccount = owner.type === "service_account";
  const OwnerIcon = isAccount ? Bot : User;
  return (
    <span className='inline-flex items-center gap-1.5'>
      <OwnerIcon className='size-3.5 shrink-0 text-muted-foreground' aria-hidden />
      <span className='sr-only'>{isAccount ? "Service account" : "Person"}:</span>
      <span className={cn(isAccount && "font-mono")}>{owner.label}</span>
    </span>
  );
}
