import { Link } from "react-router-dom";
import type { CustomApp } from "@/types/apps";
import { AppAccessPane } from "../../AppDetail/components/AppAccessPane";
import { AppInfo } from "../../AppDetail/components/AppInfo";
import { CustomDomains } from "../../AppDetail/components/CustomDomains";
import { ConsolePanel } from "../../ConsolePanel";
import { Column, TwoColumns } from "./layout";

export const SettingsPage = ({ app }: { app: CustomApp }) => (
  <div className='flex flex-col gap-3' data-testid='apps-console-page-settings'>
    <TwoColumns>
      <Column>
        <ConsolePanel
          id='access'
          title='Access'
          info='Who can open this app: the whole org, or named members and groups.'
        >
          <AppAccessPane app={app} />
        </ConsolePanel>
        <ConsolePanel id='domains' title='Custom domains'>
          <CustomDomains appId={app.id} />
        </ConsolePanel>
      </Column>
      <ConsolePanel id='identity' title='Identity & manifest'>
        <AppInfo app={app} />
      </ConsolePanel>
    </TwoColumns>
    {/* The fleet-wide views, reachable from inside an app as well as from the fleet's
        summary cards. Links, not pages of this console: they are about every app. */}
    <p className='flex flex-wrap gap-x-4 gap-y-1 text-muted-foreground text-xs'>
      <span>Across the fleet:</span>
      <Link
        to='/admin/apps/storage'
        className='underline underline-offset-2 hover:text-foreground'
        data-testid='apps-console-audit-link'
      >
        Storage &amp; retention
      </Link>
      <Link
        to='/admin/apps/access'
        className='underline underline-offset-2 hover:text-foreground'
        data-testid='apps-console-access-link'
      >
        Staff access &amp; lockdowns
      </Link>
    </p>
  </div>
);
