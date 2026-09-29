import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";
import type { Organization } from "@/types/organization";
import { crewSettingsHref, crewSettingsOrg } from "../utils";

/**
 * This browser holds no live kiosk cookie. It is also what a magic link
 * requested on a kiosk shows when it is opened on another device — the link
 * lands on `/kiosk`, and that device is not the tablet.
 */
export default function NotAKiosk({ orgs }: { orgs: Organization[] | undefined }) {
  const org = crewSettingsOrg(orgs);
  return (
    <div className='flex flex-col gap-4' data-testid='kiosk-manage-not-a-kiosk'>
      <div className='flex flex-col gap-1'>
        <h1 className='font-semibold text-xl'>This browser isn&apos;t a store tablet</h1>
        <p className='text-muted-foreground text-sm'>
          Nothing to manage here. A kiosk is managed from the tablet it runs on, or revoked from
          Settings → Organization → Crew → Kiosks — revoking works from any browser.
        </p>
      </div>
      <div className='flex flex-col gap-2 sm:flex-row'>
        {org && (
          <Button asChild variant='outline'>
            <Link to={crewSettingsHref(org)} data-testid='kiosk-manage-crew-settings'>
              Open Crew settings
            </Link>
          </Button>
        )}
        <Button asChild>
          <Link to={ROUTES.ROOT} data-testid='kiosk-manage-open-oxygen'>
            Open Oxygen
          </Link>
        </Button>
      </div>
    </div>
  );
}
