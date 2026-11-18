import type { CustomApp } from "@/types/apps";
import { Activity } from "../../AppDetail/components/Activity";
import { Availability } from "../../AppDetail/components/Availability";
import { Issues, IssuesBadge } from "../../AppDetail/components/Issues";
import { ConsolePanel } from "../../ConsolePanel";
import { InvocationsPanel } from "../Invocations";
import { PRODUCTION_ONLY, type Scope, TwoColumns } from "./layout";

export const ActivityPage = ({
  app,
  environment,
  scope,
  onOpenFunction
}: {
  app: CustomApp;
  environment: string;
  scope: Scope;
  onOpenFunction: (name: string) => void;
}) => (
  <div className='flex flex-col gap-3' data-testid='apps-console-page-activity'>
    <TwoColumns>
      <ConsolePanel
        id='observability'
        title='Availability & visitors'
        scope={PRODUCTION_ONLY}
        info='Requests and failures per window from real traffic, re-read every minute, and who opened the app. Measured on production only.'
      >
        <div className='flex flex-col gap-3'>
          <Availability app={app} />
          <Activity appId={app.id} />
        </div>
      </ConsolePanel>
      <ConsolePanel
        id='issues'
        title='Issues'
        scope={PRODUCTION_ONLY}
        info='Production failures of the last 7 days, one per function and cause, the way the pager groups them.'
        actions={<IssuesBadge appId={app.id} />}
      >
        <div className='-mx-3 -mb-3'>
          <Issues app={app} onOpenFunction={onOpenFunction} />
        </div>
      </ConsolePanel>
    </TwoColumns>
    <InvocationsPanel app={app} environment={environment} scope={scope} />
  </div>
);
