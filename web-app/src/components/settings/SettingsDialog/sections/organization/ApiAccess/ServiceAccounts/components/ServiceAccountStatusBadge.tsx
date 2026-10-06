import { Badge } from "@/components/ui/shadcn/badge";
import type { ServiceAccount } from "@/types/orgApiAccess";

/** Enabled carries the tint, since it is the state an admin wants; disabled is deliberately flat. */
export function ServiceAccountStatusBadge({
  account
}: {
  account: Pick<ServiceAccount, "disabled_at">;
}) {
  return account.disabled_at ? (
    <Badge variant='outline' className='text-muted-foreground'>
      Disabled
    </Badge>
  ) : (
    <Badge variant='outline' className='border-primary/30 bg-primary/5 text-primary'>
      Enabled
    </Badge>
  );
}

export function RoleLabel({ role }: { role: ServiceAccount["org_role"] }) {
  return <span>{role === "admin" ? "Admin" : "Member"}</span>;
}
