import { PRODUCTION } from "@/types/appEnvironments";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../../resolveBundleUrl";
import { Functions } from "../../AppDetail/components/Functions";
import { ConsolePanel } from "../../ConsolePanel";
import { AppMapPanel } from "../AppMap";
import { ChecksPanel } from "../Checks";
import { SchedulesPanel } from "../Schedules";
import { TestNotifications } from "../TestNotifications";
import { Column, type Scope, TwoColumns } from "./layout";

export const FunctionsPage = ({
  app,
  environment,
  scope,
  selected,
  onSelect,
  onOpenFunction
}: {
  app: CustomApp;
  environment: string;
  scope: Scope;
  selected: string | null;
  onSelect: (name: string | null) => void;
  onOpenFunction: (name: string) => void;
}) => (
  <div className='flex flex-col gap-3' data-testid='apps-console-page-functions'>
    <AppMapPanel
      appId={app.id}
      orgId={app.org_id}
      environment={environment}
      scope={scope}
      selected={selected}
      onOpenFunction={onOpenFunction}
    />
    <TwoColumns>
      <ConsolePanel
        id='functions'
        title='Functions'
        scope={scope}
        info='Each Oxy Function the build ships: its manifest, its recent invocations, and a Run panel that calls it in this environment.'
      >
        <Functions
          appId={app.id}
          selected={selected}
          onSelect={onSelect}
          environment={environment === PRODUCTION ? undefined : environment}
          // The canonical subpath URL, which the runtime routes `fn/<name>` under —
          // not the subdomain — resolved against this origin, so a bare path from a
          // local stack still yields a URL.
          appUrl={resolveBundleUrl(app.url)}
          appRef={app}
        />
      </ConsolePanel>
      <Column>
        <SchedulesPanel app={app} environment={environment} scope={scope} />
        <ChecksPanel app={app} environment={environment} scope={scope} />
        <TestNotifications app={app} />
      </Column>
    </TwoColumns>
  </div>
);
