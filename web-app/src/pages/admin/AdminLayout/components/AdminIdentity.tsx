import { ShieldAlert, ShieldCheck } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";

/**
 * WHO is in the admin panel, and with WHAT authority.
 *
 * The admin surface is cross-tenant: everything you do here reaches other
 * people's organizations. Operating it without a persistent "you are X, acting as
 * Y" readout is how staff misjudge their own blast radius. This is deliberately
 * the loudest element in an otherwise monochrome topbar.
 *
 * Two platform tiers (they are NOT the same, and the difference is load-bearing):
 *   - Global Owner (OXY_OWNER)  — everything, incl. Billing + Global-admin mgmt.
 *   - Global Admin (app_admins) — most of admin, but NOT Billing and NOT
 *     Global-admin management, and cannot grant a partner billing/secrets.
 */
export function AdminIdentity() {
  const { data: user } = useCurrentUser();
  if (!user) return null;

  const isOwner = !!user.is_owner;
  const isAdmin = !!user.is_app_admin;
  if (!isOwner && !isAdmin) return null;

  const role = isOwner ? "Global Owner" : "Global Admin";
  const Icon = isOwner ? ShieldAlert : ShieldCheck;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          data-testid='admin-identity'
          className={cn(
            "flex h-6 max-w-[15rem] cursor-default items-center gap-1.5 rounded border px-1.5",
            // Owner is the higher-blast-radius tier — it reads hotter on purpose.
            isOwner
              ? cn("border-warning/40", ADMIN_TONE.warn.bg, ADMIN_TONE.warn.text)
              : "border-primary/30 bg-primary/5 text-primary"
          )}
        >
          <Icon className='size-3.5 shrink-0' />
          <span className='shrink-0 font-semibold text-[11px] leading-none'>{role}</span>
          <span className='mx-0.5 h-3 w-px shrink-0 bg-current opacity-25' />
          <span className='truncate font-mono text-[11px] leading-none opacity-80'>
            {user.email}
          </span>
        </div>
      </TooltipTrigger>
      <TooltipContent side='bottom' align='end' className='max-w-xs'>
        <p className='font-medium'>
          {role} — {user.email}
        </p>
        <p className='mt-1 text-muted-foreground text-xs'>
          {isOwner
            ? "Full platform authority, including Billing and Global-admin management. You can grant partners the sensitive billing/secrets capabilities."
            : "Most of the admin surface, but NOT Billing and NOT Global-admin management. You cannot grant a partner the billing/secrets capabilities."}
        </p>
        <p className='mt-1 text-muted-foreground text-xs'>
          Everything you do here is cross-tenant and written to the audit log.
        </p>
      </TooltipContent>
    </Tooltip>
  );
}
