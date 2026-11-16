import { Building2, LogOut, Settings } from "lucide-react";
import { useEffect } from "react";
import { useNavigate } from "react-router-dom";
import OxyLogo from "@/components/OxyLogo";
import SettingsDialog from "@/components/settings/SettingsDialog";
import { useSettingsDeepLink } from "@/components/settings/SettingsDialog/useSettingsDeepLink";
import { Button } from "@/components/ui/shadcn/button";
import { useAuth } from "@/contexts/AuthContext";
import { useOrgs } from "@/hooks/api/organizations";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { releaseBodyPointerLock } from "@/libs/utils/pointerEvents";
import ROUTES from "@/libs/utils/routes";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useSettingsDialog from "@/stores/useSettingsDialog";
import type { Organization } from "@/types/organization";
import { BootstrapCommands } from "./components/BootstrapCommands";

interface Props {
  /** Resolved from the URL slug by the dispatcher, not the zustand store. */
  org: Organization;
}

/**
 * What an org with no ready workspace shows at `/:orgSlug`.
 *
 * Orgs are provisioned by the Oxygen team or a partner, and every new org gets
 * a Default workspace — so "nothing ready" means setup is still in flight, not
 * that the viewer should build something. This replaced a redirect into a
 * self-serve onboarding wizard.
 *
 * Nobody creates anything here: Oxy engineers set an org up with `oxyc` and the
 * API, so the people the server would admit (`may_provision_workspace` on
 * `GET /orgs`: Oxy staff with `operate_platform` over this org AND an Owner/Admin
 * role in it) get the exact commands instead (`BootstrapCommands`), and a customer
 * is told their Oxy team is setting their apps up. Org admins also get the
 * organization settings (crew, locations, app access), none of which need a
 * workspace. Both are display gates — the server re-checks every call.
 */
export default function OrgSetupPending({ org }: Props) {
  const navigate = useNavigate();
  const { logout } = useAuth();
  const { data: user } = useCurrentUser();
  const { data: orgs } = useOrgs();
  const openSettings = useSettingsDialog((s) => s.open);

  // `/<org>?settings=organization.crew` opens the dialog straight away.
  useSettingsDeepLink();

  // Clear a body pointer lock leaked by a dialog on the page we came from, and
  // retire the last visit's workspace: `useCurrentWorkspace` is only ever
  // written by the workspace layout, so arriving here from another org would
  // hand the settings dialog that org's Workspace group.
  useEffect(() => {
    releaseBodyPointerLock();
    useCurrentWorkspace.getState().setWorkspace(null);
  }, []);

  // The server's answer for this org (`may_provision_workspace` on `GET /orgs`),
  // not `is_owner || is_app_admin`: that is true for every platform role, and
  // the server refuses an App Operator and a grant scoped to other orgs.
  const isOrgAdmin = org.role === "owner" || org.role === "admin";
  const showBootstrapCommands = org.may_provision_workspace === true;
  const otherOrgs = (orgs ?? []).filter((o) => o.id !== org.id);

  return (
    <div
      className='flex min-h-screen w-full flex-col bg-background'
      data-testid='org-setup-pending'
    >
      <div className='flex items-center justify-between gap-4 p-6'>
        <div className='flex items-center gap-2 font-medium'>
          <OxyLogo />
          <span className='truncate text-sm'>Oxygen</span>
        </div>
        {user?.email && (
          <span className='truncate text-muted-foreground text-sm'>{user.email}</span>
        )}
      </div>

      <div className='flex flex-1 items-center justify-center px-6 pb-16'>
        <div className='flex w-full max-w-xl flex-col items-center gap-6'>
          <div className='w-full max-w-md space-y-6 rounded-2xl border bg-card p-8 text-center'>
            <div className='space-y-2'>
              <div className='mx-auto mb-4 flex size-10 items-center justify-center rounded-full bg-primary/10'>
                <Building2 className='size-5 text-primary' />
              </div>
              <h1
                className='font-serif text-2xl tracking-tight'
                data-testid='org-setup-pending-title'
              >
                {org.name} is being set up
              </h1>
              <p className='text-muted-foreground text-sm'>
                Your Oxy team is setting up your apps. They'll open here as soon as they're ready.
              </p>
            </div>

            {isOrgAdmin && (
              <div className='space-y-2'>
                <Button
                  variant='outline'
                  className='w-full'
                  onClick={() => openSettings("organization.general")}
                  data-testid='org-setup-pending-open-settings'
                >
                  <Settings className='size-4' />
                  Organization settings
                </Button>
              </div>
            )}

            {otherOrgs.length > 0 && (
              <div className='space-y-2 text-left'>
                <p className='text-muted-foreground text-xs'>Switch organization</p>
                <div className='space-y-1'>
                  {otherOrgs.map((other) => (
                    <button
                      key={other.id}
                      type='button'
                      onClick={() => navigate(ROUTES.ORG(other.slug).ROOT)}
                      data-testid={`org-setup-pending-switch-${other.slug}`}
                      className='flex w-full items-center justify-between rounded-md border bg-background p-3 text-sm transition-colors hover:bg-accent hover:text-accent-foreground'
                    >
                      <span className='font-medium'>{other.name}</span>
                      <span className='text-muted-foreground text-xs capitalize'>{other.role}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}

            <Button
              variant='ghost'
              className='w-full text-muted-foreground'
              onClick={logout}
              data-testid='org-setup-pending-log-out'
            >
              <LogOut className='size-4' />
              Log out
            </Button>
          </div>
          {showBootstrapCommands && <BootstrapCommands org={org} />}
        </div>
      </div>

      <SettingsDialog />
    </div>
  );
}
