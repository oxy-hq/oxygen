import { Check, LogOut, Plus, Settings, Shield } from "lucide-react";
import { type ComponentPropsWithoutRef, forwardRef } from "react";
import { useNavigate } from "react-router-dom";
import { useOpenSettings } from "@/components/settings/useSettingsRoute";
import { UserAvatar } from "@/components/UserAvatar";
import { focusRing, sectionLinkOff } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { useAuth } from "@/contexts/AuthContext";
import { useOrgs } from "@/hooks/api/organizations";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";

/** Org switcher section of the user menu (cloud mode only — mounted
 *  conditionally so `useOrgs` never fires in local mode). Ported from the
 *  removed TopBar. */
function OrgSwitcherGroup() {
  const navigate = useNavigate();
  const currentOrg = useCurrentOrg((s) => s.org);
  const { data: orgs } = useOrgs();
  const { data: profile } = useCurrentUser();
  // Orgs are provisioned, not self-served: only staff holding `create_orgs`
  // (what `POST /admin/orgs` requires) get the entry, and it opens the admin
  // tenant directory where that dialog lives.
  const canCreateOrgs = !!profile?.platform_capabilities?.includes("create_orgs");

  return (
    <>
      <DropdownMenuGroup className='flex flex-col gap-1'>
        {orgs?.map((org) => (
          <DropdownMenuItem
            key={org.id}
            onSelect={(e) => {
              // preventDefault skips Radix's auto-close animation; the
              // navigate below unmounts the tree cleanly so no body
              // pointer-events lock leaks onto the destination page.
              e.preventDefault();
              navigate(ROUTES.ORG(org.slug).ROOT);
            }}
            className={cn(
              "flex cursor-pointer items-center gap-2",
              currentOrg?.id === org.id && "bg-muted"
            )}
          >
            <div className='flex h-6 w-6 items-center justify-center rounded bg-primary/10 font-bold text-primary text-xs'>
              {org.name[0]?.toUpperCase()}
            </div>
            <span className='flex-1 truncate'>{org.name}</span>
            {currentOrg?.id === org.id && <Check className='h-4 w-4 text-primary' />}
          </DropdownMenuItem>
        ))}
        {canCreateOrgs && (
          <DropdownMenuItem
            className='cursor-pointer'
            onSelect={(e) => {
              // preventDefault skips Radix's auto-close so the menu unmounts
              // via the navigate instead — no leaking body pointer-events
              // lock on the destination page.
              e.preventDefault();
              navigate(`${ROUTES.ADMIN.TENANTS}?type=orgs`);
            }}
          >
            <Plus className='h-4 w-4' />
            New organization
          </DropdownMenuItem>
        )}
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
    </>
  );
}

type Profile = { name?: string | null; email?: string | null; picture?: string | null };

/** The menu's trigger. `open`: the rail shows names, so it is a row like the entries above
 *  it (Oxygen Nexus, Admin): the avatar in the marks' 20px column, then the person's name.
 *  Closed, the avatar alone. Forwards the trigger's props and ref (Radix `asChild`). */
const AccountTrigger = forwardRef<
  HTMLButtonElement,
  { open: boolean; profile?: Profile } & ComponentPropsWithoutRef<"button">
>(function AccountTrigger({ open, profile, ...props }, ref) {
  const avatar = (className: string) => (
    <UserAvatar
      name={profile?.name ?? ""}
      email={profile?.email ?? ""}
      picture={profile?.picture ?? undefined}
      className={className}
    />
  );
  if (!open) {
    return (
      <Button
        ref={ref}
        variant='ghost'
        size='icon'
        className='h-8 w-8 rounded-full'
        data-testid='rail-user-menu'
        aria-label='User menu'
        {...props}
      >
        {avatar("h-7 w-7 rounded-full")}
      </Button>
    );
  }
  return (
    <button
      ref={ref}
      type='button'
      className={cn(
        "flex h-8 w-full shrink-0 items-center gap-2 px-1.5 text-left transition-colors",
        sectionLinkOff,
        focusRing
      )}
      data-testid='rail-user-menu'
      aria-label='User menu'
      {...props}
    >
      <span className='flex size-5 shrink-0 items-center justify-center'>
        {avatar("size-5 rounded-full text-[10px]")}
      </span>
      <span className='min-w-0 flex-1 truncate text-sm'>
        {profile?.name || profile?.email || "Account"}
      </span>
    </button>
  );
});

/** Rail-bottom user menu: org switcher (cloud), Settings, Admin, Log out. `open`: drawn as
 *  a row with the person's name (`AccountTrigger`). */
export function RailUserMenu({ open = false }: { open?: boolean } = {}) {
  const navigate = useNavigate();
  const { isLocalMode, logout } = useAuth();
  const { data: profile } = useCurrentUser();
  const isAdmin = !!(profile?.is_owner || profile?.is_app_admin);
  const openSettings = useOpenSettings();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <AccountTrigger open={open} profile={profile ?? undefined} />
      </DropdownMenuTrigger>
      <DropdownMenuContent side='right' align='end' className='min-w-48'>
        {!isLocalMode && <OrgSwitcherGroup />}
        <DropdownMenuItem
          className='cursor-pointer'
          // No argument: the store remembers where you were, and its default is a
          // section every role can open. Naming one here overrode both, and named a
          // section a plain Member is not allowed to see.
          onSelect={() => openSettings()}
        >
          <Settings className='h-4 w-4' />
          Settings
        </DropdownMenuItem>
        {isAdmin && (
          <DropdownMenuItem
            className='cursor-pointer'
            onSelect={() => navigate(ROUTES.ADMIN.CUSTOMER_APPS)}
          >
            <Shield className='h-4 w-4' />
            Admin
          </DropdownMenuItem>
        )}
        {!isLocalMode && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className='cursor-pointer text-destructive focus:text-destructive'
              onClick={logout}
            >
              <LogOut className='h-4 w-4' />
              Log out
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
